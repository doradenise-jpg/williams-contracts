# TotalLocked TTL Maintenance Implementation Guide

## Overview

This document describes the implementation of a **bounded maintenance policy** for discrete `TotalLocked(token)` entries in the Williams contract. The goal is to prevent these per-token aggregate records from expiring to network archival while the wills that contribute to them remain active.

## Problem Statement

Each token with active locked balances is tracked by a persistent-storage entry `DataKey::TotalLocked(Address)`. These entries have an explicit TTL set to `BUMP_AMOUNT` (~60 days) when their values change via operations like `create_will`, `top_up`, or `cancel_will`.

However, **read-only operations do not refresh the entry's TTL**. This means:
- A will that is created and never topped up, never cancelled, and whose balances remain unchanged will eventually see its `TotalLocked` entry expire to network archival.
- The will itself can remain active indefinitely (as long as it receives periodic `check_in` calls).
- When the entry expires, it enters network history but is no longer in the hot ledger state, making queries ambiguous (archived entries are not simply "missing").

## Solution

We implement a **keepalive mechanism** that refreshes TTL on `TotalLocked(token)` entries during periodic maintenance operations:

### Key Functions

**`refresh_total_locked_ttl(env: &Env, token: &Address)`** (in `storage.rs`)
- Reads the `TotalLocked(token)` entry from persistent storage.
- If it exists and is non-zero, extends its TTL to `(LIFETIME_THRESHOLD, BUMP_AMOUNT)`.
- If it does not exist or is zero, does nothing (skips unnecessary writes).
- **Cost**: O(1) per token (one read + zero or one write).

**`refresh_total_locked_ttl_for_balances(env: &Env, balances: &Map<Address, i128>)`** (in `storage.rs`)
- Iterates over every token in a will's balances.
- Calls `refresh_total_locked_ttl` for each token.
- **Cost**: O(N) in token count, where N ≤ `MAX_TOKENS` (10).

### Integration Points

The keepalive is called in three check-in entry points:

1. **`check_in`** (single-will check-in):
   - After `save_will` and after refreshing the owner index TTL.
   - Ensures that a periodic check-in keeps all of a will's token aggregates alive.

2. **`emergency_checkin`** (grace-period recovery):
   - After `save_will` and after `bump_instance_ttl`.
   - When a will is brought back from `Triggered` to `Active` state, all its token aggregates are refreshed.

3. **`batch_check_in`** (multi-will check-in):
   - Inside the per-will loop, after `save_will`.
   - Each will in the batch has its token aggregates refreshed independently.

### Maintenance Policy Summary

| Operation | TTL Refreshed | Reason |
|-----------|---------------|--------|
| `create_will` | ✓ (via `adjust_locked_value`) | Balance changes; entry is written. |
| `top_up` | ✓ (via `adjust_locked_value`) | Balance changes; entry is written. |
| `cancel_will` | ✓ (via `adjust_locked_value`) | Balance is removed; entry is updated or deleted. |
| `release_inheritance` | ✓ (via `adjust_locked_value`) | Balance is removed; entry is updated. |
| `get_total_locked` | ✗ | Read-only; keeps cost low. |
| `get_protocol_stats` | ✗ | Read-only; keeps cost low. |
| `check_in` | ✓ (via `refresh_total_locked_ttl_for_balances`) | **New:** periodic maintenance keeps aggregates alive. |
| `emergency_checkin` | ✓ (via `refresh_total_locked_ttl_for_balances`) | **New:** recovery operation refreshes state. |
| `batch_check_in` | ✓ (via `refresh_total_locked_ttl_for_balances`) | **New:** each will refreshes its aggregates. |

## Zero/Inactive Aggregates

The `refresh_total_locked_ttl` function **does not** write if:
- The entry does not exist in persistent storage.
- The entry exists but contains a zero value (zero entries are always removed, never stored).

This prevents unnecessary writes for:
- Cancelled wills that are no longer contributing to aggregates.
- Tokens that have been released from all wills.

## Cost Analysis

### Single Check-In

- **New persistent reads**: N (one per token in the will, N ≤ 10).
- **New persistent writes**: at most N (one per token, only if entry exists and is non-zero).
- **Cost**: O(N) = O(1) constant (bounded by `MAX_TOKENS`).

### Batch Check-In (M wills)

- **New persistent reads**: M × N (each will reads its tokens).
- **New persistent writes**: at most M × N.
- **Cost**: O(M × N) = O(50 × 10) = O(500) operations worst-case.

This is acceptable because:
1. `batch_check_in` is already O(M) in will count (each will is loaded and saved).
2. The O(N) token refresh is a linear multiple of existing work.
3. The total cost scales predictably with protocol usage, not with network size.

## Testing

A comprehensive test module `total_locked_ttl_maintenance_test.rs` verifies:

1. **Single check-in refreshes TTL**: After `check_in`, the TTL of `TotalLocked(token)` entries is extended.
2. **Emergency check-in refreshes TTL**: After `emergency_checkin`, token aggregates are refreshed.
3. **Batch check-in refreshes TTL**: Each will in `batch_check_in` refreshes its token aggregates.
4. **Non-existent aggregates are skipped**: `check_in` does not create entries for tokens the will doesn't hold, nor does it touch aggregates for cancelled wills.
5. **Multi-token wills refresh all tokens**: A will holding 2+ tokens refreshes TTL on all of them.

## Operational Considerations

### Monitoring

Operators should monitor:
- **Average check-in frequency**: How often wills are kept current via `check_in` calls.
- **Token diversity**: The number of distinct tokens in the protocol (affects per-transaction cost).
- **Archive rate**: Hypothetically track how often aggregates enter/exit archival (though this is not directly observable on-chain).

### If Stellar's Block Time Changes

The TTL constants (`DAY_IN_LEDGERS`, `LIFETIME_THRESHOLD`, `BUMP_AMOUNT`) are defined in `storage.rs` with comments explaining how to adjust them. If the network's average close time drifts away from 5 seconds, these constants must be updated and the contract redeployed. The maintenance logic (this implementation) does not change — it simply calls `extend_ttl` with the same thresholds used elsewhere.

### If `MAX_TOKENS` Increases

If the per-will token limit is raised above 10, the O(N) cost scales linearly. The cost remains bounded and predictable as long as N stays in the dozens. If N grows to hundreds, the read footprint of `save_will` itself would already be a concern, and the architecture might need broader optimization.

## Backward Compatibility

- The new functions are internal helpers (`refresh_total_locked_ttl`, `refresh_total_locked_ttl_for_balances`) and are not part of the public contract API.
- Existing entry points and their behavior are unchanged; only their implementation gains the new TTL refresh calls.
- The maintenance policy is **opt-in** (by calling `check_in`) — if a will never receives check-ins, its aggregates will expire like any other inactive data.

## Related Documents

- **ADR 0002**: Describes the discrete per-token storage model and the rationale for `DataKey::TotalLocked(Address)`.
- **ADR 0003**: The full architectural decision record for this implementation, including edge cases, rationale, and future considerations.
- **`contracts/will/src/storage.rs`**: Implementation of `refresh_total_locked_ttl`, `refresh_total_locked_ttl_for_balances`.
- **`contracts/will/src/lib.rs`**: Integration points in `check_in`, `emergency_checkin`, `batch_check_in`.
- **`contracts/will/src/total_locked_ttl_maintenance_test.rs`**: Comprehensive test suite.

## Verification Against Acceptance Criteria

✓ **An active token aggregate has a documented maintenance path.**
- The maintenance path is `check_in` (or `emergency_checkin` / `batch_check_in` for special cases).
- Functions are well-documented with docstrings explaining the policy.
- This guide provides operational context.

✓ **Keeping a will current does not hide an expiring required aggregate.**
- A will kept current via periodic `check_in` calls explicitly refreshes the TTL of all its token aggregates.
- Aggregates will not expire while the will remains active and receives regular maintenance.

✓ **Zero/inactive aggregates are not renewed unnecessarily.**
- The `refresh_total_locked_ttl` function checks if the entry exists and is non-zero before writing.
- Only active, non-zero aggregates are refreshed. This prevents unnecessary writes.
