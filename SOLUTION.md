# Solution: TotalLocked TTL Maintenance Policy

## Executive Summary

This implementation addresses the issue where discrete `TotalLocked(token)` entries (per-token aggregate records) could expire to network archival independently of the wills they represent, even though those wills remain active and correct.

**Solution:** Implement a **bounded keepalive mechanism** that refreshes TTL on `TotalLocked(token)` entries during periodic maintenance operations (`check_in`, `emergency_checkin`, `batch_check_in`).

## Problem Analysis

### Root Cause
- Discrete `TotalLocked(token)` entries are created when balances change (e.g., `create_will`, `top_up`).
- Read-only operations (`get_total_locked`, `get_protocol_stats`) do not refresh TTL.
- A will with stable balances that never changes will eventually expire its `TotalLocked` entry.

### Impact
- Archived entries create ambiguity: they are not simply "missing" on a live host—they exist in network history but have passed through the archival boundary.
- While protocol statistics remain correct (reconstructed from wills at query time), the aggregate records are no longer in the hot ledger state.
- This violates operational transparency: active will data should be consistently in the live ledger state.

## Solution Design

### Core Principle
**Active maintenance keeps everything alive:** A will demonstrably kept current via periodic `check_in` calls explicitly refreshes the TTL of all its token aggregates at every check-in. This ensures that active wills keep their aggregates live.

### Key Components

#### 1. New Storage Functions

**`refresh_total_locked_ttl(env: &Env, token: &Address)`**
- Reads `TotalLocked(token)` from persistent storage.
- If it exists and is non-zero, extends its TTL to `(LIFETIME_THRESHOLD, BUMP_AMOUNT)`.
- Skips writes if the entry doesn't exist or is zero (no unnecessary overhead).
- O(1) cost per token.

**`refresh_total_locked_ttl_for_balances(env: &Env, balances: &Map<Address, i128>)`**
- Refreshes TTL for all tokens in a will's balances.
- O(N) cost where N ≤ 10 (bounded by `MAX_TOKENS`).

#### 2. Integration Points

The keepalive is called in three check-in entry points:

| Entry Point | Location | Rationale |
|-------------|----------|-----------|
| `check_in` | After `save_will` | Routine maintenance; every active will has periodic check-ins |
| `emergency_checkin` | After `bump_instance_ttl` | Recovery operation; will transitions back to Active |
| `batch_check_in` | In per-will loop after `save_will` | Batch maintenance; each will refreshes its aggregates |

#### 3. Zero/Inactive Aggregates

Completely skipped:
- If `TotalLocked(token)` doesn't exist → no write
- If the amount is zero → no write (zero entries are always removed)

This prevents unnecessary writes for cancelled wills or released tokens.

## Verification Against Acceptance Criteria

### ✓ Criterion 1: An active token aggregate has a documented maintenance path.

**Evidence:**
- Maintenance path: `check_in`, `emergency_checkin`, `batch_check_in`.
- Functions documented with comprehensive docstrings.
- ADR 0003 provides full architectural context.
- Operational guide (TOTAL_LOCKED_TTL_MAINTENANCE.md) for operators.

### ✓ Criterion 2: Keeping a will current does not hide an expiring required aggregate.

**Evidence:**
- A will kept current via periodic `check_in` explicitly refreshes TTL on all its token aggregates.
- Aggregates expire only if the will stops receiving check-ins (expected behavior).
- Test suite verifies TTL is extended on every check-in.

### ✓ Criterion 3: Zero/inactive aggregates are not renewed unnecessarily.

**Evidence:**
- `refresh_total_locked_ttl` checks existence and non-zero before writing.
- Cancelled wills don't have their aggregates refreshed (not in `will.balances`).
- Zero entries are never written (always removed via `adjust_locked_value`).
- No wasteful writes are performed.

## Cost Analysis

### Complexity
- **Time**: O(N) per check-in where N ≤ 10 (bounded by MAX_TOKENS).
- **Space**: No additional persistent storage required.
- **Network**: Each token refresh is one extend_ttl call (negligible cost).

### Single Check-In
- Persistent reads: N (one per token)
- Persistent writes: ≤ N (only if entry exists and is non-zero)
- Marginal cost: O(1) constant

### Batch Check-In (M wills)
- Operations: M × N (each will refreshes its tokens)
- Worst case: 50 wills × 10 tokens = 500 operations
- Already O(M) for other per-will work, so O(N) is acceptable addition

### Bounded Maintenance
- Cost scales predictably with protocol usage, not network size.
- No exponential blowup as token diversity grows.

## Implementation Details

### Files Modified
1. **`contracts/will/src/storage.rs`**
   - Added `refresh_total_locked_ttl` function
   - Added `refresh_total_locked_ttl_for_balances` function

2. **`contracts/will/src/lib.rs`**
   - Updated `check_in` to call `refresh_total_locked_ttl_for_balances`
   - Updated `emergency_checkin` to call `refresh_total_locked_ttl_for_balances`
   - Updated `batch_check_in` to call `refresh_total_locked_ttl_for_balances` per will
   - Added test module declaration for `total_locked_ttl_maintenance_test`

### Files Added
1. **`contracts/will/src/total_locked_ttl_maintenance_test.rs`**
   - 5 comprehensive test cases verifying TTL refresh behavior

2. **`docs/adr/0003-total-locked-ttl-maintenance.md`**
   - Full architectural decision record

3. **`docs/TOTAL_LOCKED_TTL_MAINTENANCE.md`**
   - Operational guide for maintenance and monitoring

## Testing

Comprehensive test suite covers:
1. Single check-in refreshes TTL on all tokens
2. Emergency check-in refreshes TTL on all tokens
3. Batch check-in refreshes TTL on all tokens for each will
4. Non-existent aggregates are not unnecessarily refreshed
5. Multi-token wills refresh all tokens independently

All tests verify that:
- TTL is extended to a future deadline after refresh
- Entry is not created if it didn't exist
- Zero entries are not refreshed

## Backward Compatibility

- ✓ No breaking changes to public API
- ✓ New functions are internal helpers only
- ✓ Maintenance is opt-in (by calling `check_in`)
- ✓ No data migration required
- ✓ Existing behavior unchanged

## Edge Cases Handled

1. **Will never checked in**: Aggregates expire naturally (expected).
2. **Multi-token will**: All aggregates refreshed (verified in tests).
3. **Cancelled will**: Aggregates for removed tokens not refreshed (expected).
4. **Batch operations**: Each will independently refreshes (tested).
5. **Multiple wills sharing token**: Each refreshes independently; last write wins (correct).

## Operational Recommendations

### Monitoring
- Track average check-in frequency per active will
- Monitor token diversity (number of distinct tokens in protocol)
- Compare cost of check-in with/without TTL refresh (likely negligible)

### If Stellar Block Time Changes
- Update `DAY_IN_LEDGERS`, `LIFETIME_THRESHOLD`, `BUMP_AMOUNT` in `storage.rs`
- Maintenance logic requires no changes (just calls same TTL constants)
- Redeploy contract with updated constants

### If `MAX_TOKENS` Increases
- Cost scales linearly; still bounded and predictable
- No architectural change needed unless N exceeds dozens
- Monitor read footprint of `save_will` as N grows

## Related Documentation

- **ADR 0002**: Discrete per-token storage model (DataKey::TotalLocked)
- **ADR 0003**: This implementation's full architectural decision
- **TOTAL_LOCKED_TTL_MAINTENANCE.md**: Operational guide
- **Test suite**: `total_locked_ttl_maintenance_test.rs`

## Conclusion

This implementation provides a **simple, bounded, and transparent** solution to the TotalLocked TTL maintenance problem:

- **Simple**: Just refresh TTL during routine check-ins.
- **Bounded**: O(N) cost where N ≤ 10; scales with existing work.
- **Transparent**: Active will data consistently in hot ledger state; archived entries are no longer ambiguous.

All acceptance criteria are met, and the solution is production-ready.
