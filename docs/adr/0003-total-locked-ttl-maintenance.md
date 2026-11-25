# ADR 0003: TotalLocked TTL Maintenance Policy

## Status

Accepted and Implemented.

Discrete `TotalLocked(token)` entries are refreshed during active maintenance operations (`check_in`, `emergency_checkin`, `batch_check_in`) to prevent network archival of token aggregate records while the wills that contribute to them remain active.

## Context

### The Problem

Each token with active locked balances is tracked via a discrete persistent-storage entry `DataKey::TotalLocked(Address)` (see ADR 0002). These entries have an explicit TTL set by the contract, initially set to `BUMP_AMOUNT` (~60 days at the current 5-second Stellar block time) when the balance changes.

However, **read-only operations do not refresh the entry's TTL**:

- `get_total_locked` reads the value without touching TTL.
- `get_protocol_stats` reconstructs aggregates from `TotalLocked` entries without refreshing them.
- `check_in` and other maintenance calls update the will's state but do not refresh the aggregate.

**Consequence:** A long-lived will with a stable token balance (never topped up, never cancelled, balances unchanged) will see its `TotalLocked` entry expire to Soroban network archival independently of the will itself, even though the will remains active and the protocol's internal statistics are still correct.

### The Impact

When a `TotalLocked(token)` entry expires and enters network archival:

1. **Queries still work**: `get_protocol_stats` reconstructs the aggregate by walking every will and summing live balances. The returned statistics are correct even if the entry is archived.
2. **Writes trigger restoration**: The next `adjust_locked_value` call (from `create_will`, `top_up`, `cancel_will`, or `release_inheritance`) will read the archived entry as "not found" (zero), compute a new total, and restore the entry with a fresh TTL.
3. **Transparency suffers**: An on-chain observer or audit tool cannot distinguish between "the aggregate was correct all along but archived" and "the aggregate was lost". The entry's absence becomes ambiguous.

This is not a correctness bug, but it is a data durability and transparency issue: **archived entries are not simply missing on a live host** — they exist in network history but have passed through the archival boundary and are no longer in the hot ledger state.

## Decision Drivers

- Soroban's storage system automatically archives entries whose TTL expires without being refreshed. This is a network feature, not a contract choice.
- An actively maintained will should keep all its data live, including the aggregate totals for its tokens.
- Keeping aggregates alive prevents the need for restoration on the first balance update after a long stability period.
- The bounded cost of keepalive (O(N) in token count, N ≤ 10) is negligible compared to the cost of a state transition.
- Operational transparency: observers should see active will data consistently in the live ledger state.

## Solution: Bounded Maintenance Policy

### Core Rules

1. **Write operations** (`create_will`, `top_up`, `cancel_will`, `release_inheritance`, `distribute`) automatically refresh TTL via `adjust_locked_value`. No change needed.

2. **Read-only operations** (`get_total_locked`, `get_protocol_stats`) remain read-only and do not refresh. This keeps them lightweight.

3. **Active maintenance operations** (`check_in`, `emergency_checkin`, `batch_check_in`) refresh TTL on all `TotalLocked(token)` entries for every token the will holds via the new `refresh_total_locked_ttl_for_balances` function.

4. **Zero/inactive aggregates** are skipped:
   - If `TotalLocked(token)` does not exist in persistent storage, no write is performed.
   - If the amount is zero, no write is performed (zero entries are always removed, never stored).

### Why Check-In Is the Right Hook

- **Frequency**: `check_in` is a **routine**, owner-initiated operation (called periodically to prove activity).
- **Semantics**: A check-in explicitly signals that the will and all its data are still active and relevant.
- **Availability**: By requiring a check-in to reach `Active` status (after `create_will`, after `emergency_checkin`), every active will has at least periodic check-ins; no will is kept alive *only* by external events.
- **Consistency**: The will itself is explicitly refreshed (via `save_will`), so it makes sense to refresh its aggregates at the same moment.
- **Cost**: The O(N) token refresh is a linear multiple of an operation that already costs O(1) state writes per token (via `save_will`), so the marginal overhead is bounded and predictable.

### Implementation

#### New Functions in `storage.rs`

**`refresh_total_locked_ttl(env: &Env, token: &Address)`**
- Reads the `TotalLocked(token)` entry from persistent storage.
- If it exists and is non-zero, extends its TTL to `(LIFETIME_THRESHOLD, BUMP_AMOUNT)`.
- If it does not exist or is zero, does nothing.
- **Cost**: O(1) per token (one read, zero or one write).

**`refresh_total_locked_ttl_for_balances(env: &Env, balances: &Map<Address, i128>)`**
- Iterates over every token in the will's `balances` map.
- Calls `refresh_total_locked_ttl` for each token.
- **Cost**: O(N) in the number of tokens, where N ≤ `MAX_TOKENS` (10).

#### Calls in `lib.rs`

- **`check_in`**: After `save_will` and after refreshing the owner index TTL, call `refresh_total_locked_ttl_for_balances` for the will's balances.
- **`emergency_checkin`**: After `save_will` and after `bump_instance_ttl`, call `refresh_total_locked_ttl_for_balances`.
- **`batch_check_in`**: Inside the per-will loop, after `save_will`, call `refresh_total_locked_ttl_for_balances`.

## Verification Against Acceptance Criteria

### ✓ Criterion 1: An active token aggregate has a documented maintenance path.

**Met.** This ADR documents the maintenance policy:
- Aggregates are refreshed during `check_in`, `emergency_checkin`, and `batch_check_in`.
- Calls to `refresh_total_locked_ttl_for_balances` are placed consistently after will state is persisted.
- The functions `refresh_total_locked_ttl` and `refresh_total_locked_ttl_for_balances` include comprehensive docstrings explaining the policy.

### ✓ Criterion 2: Keeping a will current does not hide an expiring required aggregate.

**Met.** A will kept current via periodic `check_in` calls explicitly refreshes the TTL of all its token aggregates at every check-in. The aggregates will not expire while the will remains active and receives regular maintenance.

### ✓ Criterion 3: Zero/inactive aggregates are not renewed unnecessarily.

**Met.** The `refresh_total_locked_ttl` function checks if the entry exists and is non-zero before writing. Only active, non-zero aggregates are refreshed. This prevents unnecessary writes to aggregates that have been cancelled or released.

## Edge Cases and Notes

### Case 1: Will created, never topped up, never checked in

A will that is created and immediately expires without maintenance (or in test scenarios that bypass check-in) will have its aggregate expire along with the will. This is correct behavior — an inactive will's data can be archived.

### Case 2: Will with multi-token balances

A will holding 10 distinct tokens will refresh all 10 aggregates at every check-in. The cost is O(10) = O(1) constant and is negligible compared to the cost of `save_will`.

### Case 3: Tokens added/removed via top-up/cancellation

- **Top-up**: Adds an amount to a token. If the token is new, `adjust_locked_value` creates the entry with fresh TTL. At the next check-in, `refresh_total_locked_ttl_for_balances` will see the token in the will's `balances` map and refresh it (even though it was just created).
- **Cancellation**: Removes a token entirely from the will. The aggregate is not part of `will.balances` anymore, so `refresh_total_locked_ttl_for_balances` will not touch it. It will expire naturally if not updated by other wills' operations.

### Case 4: Multiple wills holding the same token

Each will refreshes the same `TotalLocked(token)` entry independently during its own check-in. The entry's TTL is extended by whichever will's check-in happens last (and thus calls `extend_ttl` last). This is fine — Soroban's `extend_ttl` is idempotent and sets an absolute deadline, not a relative one.

### Case 5: Guardian releases and balance changes

When a guardian release (`guardian_trigger`, `release_inheritance`, `distribute`) happens, it calls `adjust_locked_value` for every token being released, which refreshes TTL. No separate keepalive is needed there.

## Maintenance and Future Considerations

### Operational Monitoring

Operators should monitor:
- Average `check_in` frequency per active will (via events).
- If wills frequently go un-checked-in and trigger, the aggregate TTL refresh is moot (the aggregates will update when balances are released anyway).

### If Stellar's Block Time Changes

The TTL constants (`DAY_IN_LEDGERS`, `LIFETIME_THRESHOLD`, `BUMP_AMOUNT`) are defined in `storage.rs` and explained in comments. If the network's average close time drifts away from 5 seconds, these constants must be updated and the contract redeployed. The TTL refresh logic (this ADR) does not depend on absolute time — it just calls `extend_ttl` with the same thresholds used elsewhere, so no logic change is needed.

### If `MAX_TOKENS` Increases

If the per-will token limit is raised above 10, the O(N) cost of `refresh_total_locked_ttl_for_balances` scales linearly. The cost remains bounded and predictable. No architectural change is needed unless `N` exceeds dozens, at which point the read footprint of `save_will` itself would already be a concern.

## Related Issues and Documents

- **ADR 0002: `total_locked_by_token` — Bounded Vec vs. Unbounded Growth**: Describes the discrete per-token storage model and the rationale for `DataKey::TotalLocked(Address)`.
- **`contracts/will/src/storage.rs`**: Implementation of `refresh_total_locked_ttl`, `refresh_total_locked_ttl_for_balances`.
- **`contracts/will/src/lib.rs`**: Calls to `refresh_total_locked_ttl_for_balances` in `check_in`, `emergency_checkin`, `batch_check_in`.
- **Soroban Storage Archival**: See Stellar Soroban documentation on ledger TTL and archival.

