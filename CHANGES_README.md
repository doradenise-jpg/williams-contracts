# Changes: TotalLocked TTL Maintenance Policy

## What Changed?

This implementation adds a **TTL maintenance mechanism** for discrete `TotalLocked(token)` entries to prevent them from expiring to network archival while wills remain active.

## Why?

Discrete `TotalLocked(token)` entries track per-token locked balances. These entries are refreshed (TTL extended) only when balances change. A will with stable balances could see its `TotalLocked` entry expire to network archival independently of the will itself, creating operational ambiguity and potential durability issues.

## Solution

Refresh TTL on `TotalLocked(token)` entries during periodic maintenance operations:
- `check_in` (single-will check-in)
- `emergency_checkin` (grace-period recovery)
- `batch_check_in` (multi-will check-in)

## Changes Summary

### Code (Modified Files)

**`contracts/will/src/storage.rs`**
- Added `refresh_total_locked_ttl(env, token)` function
- Added `refresh_total_locked_ttl_for_balances(env, balances)` function

**`contracts/will/src/lib.rs`**
- Updated `check_in` to call `refresh_total_locked_ttl_for_balances`
- Updated `emergency_checkin` to call `refresh_total_locked_ttl_for_balances`
- Updated `batch_check_in` to call `refresh_total_locked_ttl_for_balances` per will
- Added test module declaration

### Code (New Files)

**`contracts/will/src/total_locked_ttl_maintenance_test.rs`**
- Comprehensive test suite with 5 test cases

### Documentation (New Files)

**`docs/adr/0003-total-locked-ttl-maintenance.md`**
- Full architectural decision record

**`docs/TOTAL_LOCKED_TTL_MAINTENANCE.md`**
- Operational guide for maintenance and monitoring

**`SOLUTION.md`**
- Detailed solution explanation

**`IMPLEMENTATION_SUMMARY.md`**
- Change summary and verification checklist

**`VERIFICATION_GUIDE.md`**
- How to verify the implementation

## Impact

### Performance
- **Single check-in**: O(1) constant cost (N ≤ 10 tokens per will)
- **Batch check-in**: O(M × N) where M ≤ 50 and N ≤ 10
- **Cost**: Bounded and predictable; scales with existing work

### Behavior
- **Active wills**: Token aggregates stay alive as long as will is active (maintained via check-in)
- **Inactive wills**: Aggregates expire naturally (expected behavior)
- **Zero/cancelled tokens**: Not unnecessarily refreshed (skipped by function)

### API
- **No breaking changes**: All changes are internal
- **Backward compatible**: No data migration required
- **Maintenance is opt-in**: By calling `check_in`

## Verification

See `VERIFICATION_GUIDE.md` for comprehensive verification steps:

1. ✓ Code changes in place
2. ✓ Documentation complete
3. ✓ Test coverage (5 test cases)
4. ✓ All acceptance criteria met

Quick check:
```bash
# Verify new functions exist
grep "pub fn refresh_total_locked_ttl" contracts/will/src/storage.rs

# Verify integration in check_in
grep -A 3 "refresh_total_locked_ttl_for_balances" contracts/will/src/lib.rs | head -10

# Verify test module exists
ls contracts/will/src/total_locked_ttl_maintenance_test.rs

# Verify documentation
ls docs/adr/0003-total-locked-ttl-maintenance.md
ls docs/TOTAL_LOCKED_TTL_MAINTENANCE.md
```

## Reading Guide

**Start here:**
1. `SOLUTION.md` — Overview of the problem and solution
2. `VERIFICATION_GUIDE.md` — How to verify the implementation

**For operators:**
- `docs/TOTAL_LOCKED_TTL_MAINTENANCE.md` — Maintenance and monitoring guide

**For architects:**
- `docs/adr/0003-total-locked-ttl-maintenance.md` — Full ADR with rationale and design decisions

**For developers:**
- `contracts/will/src/storage.rs` — Implementation of refresh functions
- `contracts/will/src/lib.rs` — Integration points (search for `refresh_total_locked_ttl_for_balances`)
- `contracts/will/src/total_locked_ttl_maintenance_test.rs` — Comprehensive test suite

## Key Properties

✓ **Simple**: Just refresh TTL during routine check-ins
✓ **Bounded**: O(N) cost where N ≤ 10; scales with existing work
✓ **Transparent**: Active will data consistently in hot ledger state
✓ **Zero-cost for inactive**: Non-existent and zero aggregates are skipped
✓ **Backward compatible**: No breaking changes

## Questions?

- **Why this design?** → See ADR 0003
- **How to operate?** → See TOTAL_LOCKED_TTL_MAINTENANCE.md
- **How to verify?** → See VERIFICATION_GUIDE.md
- **How does it work?** → See code comments and docstrings
