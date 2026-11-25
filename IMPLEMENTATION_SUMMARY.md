# Implementation Summary: TotalLocked TTL Maintenance Policy

## Problem

Discrete `TotalLocked(token)` entries track per-token locked balances across the protocol. These entries have an explicit TTL set only when their values change (via `create_will`, `top_up`, `cancel_will`, `release_inheritance`).

A will with stable token balances that remain unchanged will eventually see its `TotalLocked` entry expire to network archival, even though the will itself remains active. This creates ambiguity: archived entries are not simply "missing on a live host" — they exist in network history but have passed through the archival boundary.

## Solution

Implement a **bounded keepalive mechanism** that refreshes TTL on `TotalLocked(token)` entries during periodic maintenance operations (`check_in`, `emergency_checkin`, `batch_check_in`).

## Changes Made

### 1. Storage Module (`contracts/will/src/storage.rs`)

**New Functions:**

- **`refresh_total_locked_ttl(env: &Env, token: &Address)`**
  - Refreshes the TTL of a single `TotalLocked(token)` entry if it exists and is non-zero.
  - Cost: O(1) per token (one read + zero or one write).
  - Comprehensive docstring explaining the maintenance policy.

- **`refresh_total_locked_ttl_for_balances(env: &Env, balances: &Map<Address, i128>)`**
  - Refreshes TTL for all tokens in a will's balances.
  - Cost: O(N) where N ≤ `MAX_TOKENS` (10).
  - Intended for use in check-in operations.

### 2. Contract Entry Points (`contracts/will/src/lib.rs`)

**Updated Functions:**

- **`check_in`**: After `save_will` and after refreshing the owner index TTL, now calls `refresh_total_locked_ttl_for_balances(&env, &will.balances)`.

- **`emergency_checkin`**: After `save_will` and after `bump_instance_ttl`, now calls `refresh_total_locked_ttl_for_balances(&env, &will.balances)`.

- **`batch_check_in`**: Inside the per-will loop, after `save_will`, now calls `refresh_total_locked_ttl_for_balances(&env, &will.balances)` for each will.

### 3. Documentation

**New ADR:**
- **`docs/adr/0003-total-locked-ttl-maintenance.md`**: Full architectural decision record covering context, decision drivers, the solution, verification against acceptance criteria, edge cases, and future considerations.

**New Operational Guide:**
- **`docs/TOTAL_LOCKED_TTL_MAINTENANCE.md`**: Implementation guide for operators, including cost analysis, monitoring recommendations, and backward compatibility notes.

### 4. Tests

**New Test Module:**
- **`contracts/will/src/total_locked_ttl_maintenance_test.rs`**: Comprehensive test suite verifying:
  - `check_in` refreshes TTL on all token aggregates.
  - `emergency_checkin` refreshes TTL on all token aggregates.
  - `batch_check_in` refreshes TTL on all token aggregates for each will.
  - Non-existent and zero aggregates are not unnecessarily refreshed.
  - Multi-token wills refresh all tokens.

The test module is integrated into the test suite via a new module declaration in `lib.rs`.

## Acceptance Criteria Met

✓ **An active token aggregate has a documented maintenance path.**
- Maintenance occurs via `check_in`, `emergency_checkin`, and `batch_check_in`.
- Functions and ADRs provide clear documentation.

✓ **Keeping a will current does not hide an expiring required aggregate.**
- Active maintenance keeps all token aggregates alive via periodic TTL refresh.
- Aggregates expire only if the will stops receiving check-ins.

✓ **Zero/inactive aggregates are not renewed unnecessarily.**
- `refresh_total_locked_ttl` skips entries that don't exist or are zero.
- No wasteful writes are performed.

## Cost and Performance

### Single Check-In
- **New operations**: N persistent reads + ≤ N persistent writes (N ≤ 10).
- **Cost**: O(1) constant (bounded by token count).

### Batch Check-In (M wills)
- **New operations**: M × N persistent operations.
- **Worst case**: 50 wills × 10 tokens = 500 operations.
- **Cost**: Scales linearly with existing work, negligible marginal overhead.

### Zero/Inactive Aggregates
- Completely skipped, no unnecessary writes.

## Backward Compatibility

- New functions are internal helpers only.
- Existing public API unchanged.
- Maintenance is opt-in (by calling `check_in`).
- No data migration required.

## Related ADRs and Documents

- **ADR 0002**: Discrete per-token storage model.
- **ADR 0003**: This implementation's full decision record.
- **TOTAL_LOCKED_TTL_MAINTENANCE.md**: Operational guide.

## Files Modified

- `contracts/will/src/storage.rs`: New functions `refresh_total_locked_ttl`, `refresh_total_locked_ttl_for_balances`.
- `contracts/will/src/lib.rs`: Integrated TTL refresh calls in `check_in`, `emergency_checkin`, `batch_check_in`. Added test module declaration.

## Files Added

- `contracts/will/src/total_locked_ttl_maintenance_test.rs`: Test suite.
- `docs/adr/0003-total-locked-ttl-maintenance.md`: Architectural decision record.
- `docs/TOTAL_LOCKED_TTL_MAINTENANCE.md`: Operational guide.
