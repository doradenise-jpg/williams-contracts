# ADR 0002: Legacy Single-Token `token`/`balance` Mirror Alongside `balances`

## Status

Accepted (current implementation), with known desync risk documented below.
No migration is scheduled; this ADR records the tradeoff so future
contributors can tell "intentional legacy shim" from "bug."

## Context

`Will` stores locked funds two ways at once:

- `balances: Map<Address, i128>` — the authoritative multi-token source of
  truth, populated from the `tokens: Vec<(Address, i128)>` list passed to
  `create_will`.
- `token: Address` and `balance: i128` — a single-token mirror whose token is
  the **first** entry of `tokens` and whose amount is **derived from the
  accumulated `balances` map** (`balances.get(primary_token)`), so the mirror
  can never disagree with `balances[token]`. `create_will` also rejects a
  `tokens` list containing the same address twice with
  `WillError::DuplicateToken` (#350), since a duplicate would make "first
  entry" an ambiguous definition for the mirror.

Multi-token support (`balances`) was added after the contract's original
single-token design (`token`/`balance`). Rather than migrating every entry
point to the map in one pass, `token`/`balance` were kept as a "primary
token" mirror so that single-token helpers didn't need to be rewritten
immediately.

## Why keep the mirror instead of migrating everything at once

- **Several entry points were never generalized to multi-token and still
  read only the mirror**: `reveal_and_claim` computes a claimant's share as
  `will.balance * percentage / 100` and transfers via `will.token` alone,
  never touching `balances`. Removing the mirror without first rewriting
  these to iterate `balances` would silently break every will with more than
  one locked token at these entry points. (`cancel_will` was migrated to
  refund and decrement locked value for every entry in `balances` in #353,
  so it no longer depends on the mirror for its payouts.)
- **`merge_wills` and `split_will` also key off the mirror** for their
  headline balance math (`combined_balance = will_a.balance +
  will_b.balance` in `merge_wills`), while separately combining the
  `balances` map. Both need to stay populated for these entry points to
  produce a will whose `balance` field means anything at all.
- **The alternative — migrating `cancel_will`, `reveal_and_claim`,
  `merge_wills`, and `split_will` to be fully multi-token-aware in the same
  change that introduced `balances`** — was judged too large and risky to
  land atomically; the mirror let multi-token support ship for `create_will`
  and `top_up` first, with the remaining entry points migrated
  incrementally (or left on the mirror, for single-token wills, which are
  still the common case).

## Known consequences (the desync risk)

> **Historical note (superseded):** The consequences below describe the
> state of the code when this ADR was written. Item 1 in particular is no
> longer accurate — see "Current state correction (2024-06-01)" at the end
> of this document. The text is preserved here to record the original
> tradeoff and the risk that motivated later work.

Because `token`/`balance` are a snapshot of *only the first token*, not a
derived total, they can and do drift from `balances` in ways that are easy
to misread as "the will's balance":

1. **`reveal_and_claim` never updates `balances`.** *(Historical — no longer
   true; superseded by multi-token claim delivery, issue #18. See the
   current-state correction below.)* It debited `will.balance` only. For a
   will with more than one locked token, `balances` still showed the
   pre-claim amount for the primary token after a claim — `will.balance`
   and `balances.get(will.token)` disagreed.
2. **`merge_wills`' `combined_balance` is not "total value across all
   tokens."** It is `will_a.balance + will_b.balance`, i.e. primary-token
   amounts only, summed. For wills holding multiple tokens, this sum has no
   coherent meaning as a single `i128` — it is not the total locked value,
   just an arithmetic combination of two single-token snapshots.
3. **Any future entry point that reads `will.balance` as "the will's total
   locked value"** (a natural-sounding but incorrect assumption for a
   multi-token will) will silently under- or over-count. `get_will` returns
   both fields, so nothing in the type system flags this.

These are latent risks rather than exploitable bugs in the currently shipped
entry points (each function has consistently used the same field for both
its read and write side so far), but they are exactly the kind of "quirk vs.
oversight" ambiguity this ADR exists to resolve: **the mirror is only
guaranteed accurate for the primary token; any code path that needs a
will's true total locked value, across every token, must sum `balances`,
never read `will.balance`.**

## Migration plan

No migration is scheduled. If/when `cancel_will`, `reveal_and_claim`,
`merge_wills`, and `split_will` are generalized to operate over every entry
in `balances` rather than the primary-token mirror, `token`/`balance` can be
deprecated and eventually removed from `Will` in a version bump that updates
`spec/` and the SDK. Until then:

- Treat `token`/`balance` as **read-only, primary-token-only** convenience
  fields for single-token wills and off-chain display purposes.
- Any new entry point that needs a will's true total value must sum
  `balances`, not read `will.balance`.

## Current state correction (2024-06-01)

This section records the current behavior of the code and supersedes the
historical claims above where they conflict. It is dated so readers can
distinguish the original architectural account from the present
implementation.

### `reveal_and_claim` now delivers every token and keeps the mirror in sync

The historical statement that `reveal_and_claim` "never updates `balances`"
and pays only `will.token` is **obsolete**. `reveal_and_claim` now builds a
multi-token transfer plan: it iterates the will's `balances` map, computes
each claimant's share per token, and transfers each token to the claimant.
It updates the claim map and the legacy `token`/`balance` mirror together,
so `will.balance` and `balances.get(will.token)` remain consistent after a
claim. Multi-token claim delivery was introduced in issue #18.

### The actual current mirror invariant

`token`/`balance` is the **primary-token mirror**: `token` is the first
entry of the `tokens` list and `balance` is derived from
`balances.get(primary_token)`. It is not an independent snapshot and is not
a total across tokens. The mirror is kept in sync with `balances` by:

- `create_will` — initializes both from the `tokens` list (rejecting
  duplicate token addresses with `WillError::DuplicateToken`, #350).
- `top_up` — increments both the `balances` entry and the mirror when the
  topped-up token is the primary token.
- `split_will` — maintains the mirror consistently with the split
  `balances` map.
- `reveal_and_claim` — updates the claim map and the mirror together, as
  described above.
- `cancel_will` — refunds and decrements locked value for **every** entry
  in `balances` (#353); it does not depend on the mirror for payouts.

`merge_wills` still keys its headline balance math off the mirror
(`combined_balance = will_a.balance + will_b.balance`) while separately
combining the `balances` map, subject to the documented supported merge
policy. That combined figure is a primary-token arithmetic sum only.

### Numerical sums are not economic value

Summing raw amounts across tokens (whether from `balances` or from the
mirror) yields only a **numerical total**, not an economic valuation.
Unlike token units are not commensurable: adding a quantity of token A to a
quantity of token B produces a meaningless `i128` for any economic
comparison. Any true total-value computation must sum `balances` per token
and value each token separately (e.g. via an external price source) before
combining. The ADR does not, and must not be read to, imply that raw
cross-token sums form a valuation.
