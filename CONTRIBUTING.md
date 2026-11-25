# Contributing to williams-contracts

This repo participates in the **Stellar Wave Program** on [Drips](https://drips.network/wave). Contribution work is tied to issues that maintainers tag for an active Wave, and contributors earn rewards proportional to the Points assigned to the issues they resolve.

## Ground rules

- **Do not start work on any issue until you have been assigned by the maintainer.** Applying to an issue does not mean you're assigned — wait for confirmation (via the Drips Wave dashboard or a direct assignment on GitHub) before opening a PR.
- Keep PRs scoped to the issue they resolve. Unrelated changes slow down review and can cost you the Wave window.
- Be responsive during an active Wave — issues must be resolved before the Wave ends for Points to be awarded.

## Branch naming

Use the issue number in your branch name:

```
feat/N-short-description
fix/N-short-description
```

For example: `feat/42-guardian-quorum-check` or `fix/17-checkin-deadline-rounding`.

## Pull requests

- Your PR description must reference the issue it resolves (e.g. `Closes #42`).
- Complete the [Before opening a PR](#before-opening-a-pr) checklist before requesting review.
- Add or update unit tests for any behavior change in `contracts/will/src/test.rs`.
- If you change validation rules or add an entry point, extend the fuzzing
  harness too — see [docs/FUZZING.md](./docs/FUZZING.md#adding-a-target).
- If your PR changes contract behavior (new entrypoint, validation change,
  event/error schema change, etc.), add an entry under `[Unreleased]` in
  [CHANGELOG.md](./CHANGELOG.md) describing it in the "Added" / "Changed" /
  "Fixed" section that fits. Pure docs/tooling/test-only PRs don't need one.
  A rustdoc-only edit to an entry point or type (prose, examples, wording)
  counts as a pure docs change for this rule — see
  [ABI compatibility policy](#abi-compatibility-policy).
- If your PR adds a new `WillError` variant, add a row for it to the
  [error code table in the README](./README.md#error-codes) in the same PR —
  don't let the table and `contracts/will/src/errors.rs` drift apart.
- If your PR changes a public entry-point signature or a shared type
  (`Will`, `Beneficiary`, `Guardian`, `WillStatus`, `WillError`, ...), bump
  the crate `version` in `contracts/will/Cargo.toml` and regenerate the
  contract spec artifact — see [spec/README.md](./spec/README.md). "Signature"
  here means the structural ABI: parameter names, types and order, return
  types, and the shape of shared types. It does **not** include rustdoc text;
  see [ABI compatibility policy](#abi-compatibility-policy).

## ABI compatibility policy

The contract spec artifact (`contracts/will/src/fixtures/create_will_spec.txt`,
compared byte-for-byte by `contracts/will/src/test_xdr_spec.rs`) embeds both
the structural signature and the rustdoc bytes of each entry point. The
fixture guard therefore distinguishes two kinds of drift, and reviewers must
classify a fixture diff as one of them:

- **Structural ABI drift** — any change to a parameter name, parameter type,
  parameter order, return type, or the shape of a shared type. This is an
  interface change: bump the crate `version` in `contracts/will/Cargo.toml`,
  bump `CONTRACT_VERSION` if behavior changes, and regenerate the spec
  artifact. It requires interface coordination with integrators.
- **Documentation drift** — a prose-only rustdoc edit that leaves every
  structural field identical. This is a documentation artifact update: it
  regenerates the spec fixture (so the diff stays visible in review) but does
  **not** require a version bump, a CHANGELOG entry, or interface
  coordination. It falls under the "Pure docs/tooling/test-only" exemption
  above.

When a PR touches rustdoc on an entry point, state in the PR description that
the fixture change is documentation-only, and confirm the structural fields
are unchanged. The XDR spec test asserts this distinction: fixtures that
differ only in docs pass the structural guard, while a change to a parameter
name, type or order fails it.

## Contract versioning

`CONTRACT_VERSION` in `contracts/will/src/lib.rs` is a `u32` constant that is
compiled into the deployed Wasm and returned by the `get_contract_version`
entry point. Because the value is baked into the binary, it is **immutable
after deployment**: once a contract is deployed, its on-chain version can only
change by deploying a new binary. SDKs and integrators rely on this value to
detect behavioral changes, so the constant in source must always match the
version of the binary that is actually deployed.

### Bumping the version

Bump `CONTRACT_VERSION` in the **same PR** as any change that alters contract
behavior — a new entry point, a validation change, an event/error schema
change, or a storage schema change. Do not bump it for docs, tooling, or
test-only changes.

1. Increment `CONTRACT_VERSION` in `contracts/will/src/lib.rs`.
2. Bump the crate `version` in `contracts/will/Cargo.toml` to match.
3. Add a `[Unreleased]` entry in [CHANGELOG.md](./CHANGELOG.md) describing the
   behavioral change.
4. Regenerate the contract spec artifact — see [spec/README.md](./spec/README.md).
5. Tag the release with a matching git tag (e.g. `v1.2.0`) so the deployed
   binary can be traced back to the source that produced it.

### Keeping source and deployment in sync

A build-time check (`.github/scripts/check-contract-version.sh`) verifies
that `CONTRACT_VERSION` matches the crate `version` in
`contracts/will/Cargo.toml` and, when a git tag is present, the tag itself.
Run it locally before opening a PR:

```sh
./.github/scripts/check-contract-version.sh
```

The [Test & Build workflow](./.github/workflows/test.yml) runs the version
check and, for pull requests, checks that `CHANGELOG.md` changes when
`CONTRACT_VERSION` changes. The changelog guard compares the PR merge
checkout with the base commit from the pull request event. Reviewers still
verify that behavioral changes include a version bump.

## Storage schema versioning

Every persisted `Will` has a `schema_version` field. Soroban decodes a
`#[contracttype]` struct strictly by field name. If you add, remove or rename
a field on `Will`, every will stored by an older contract version stops
decoding unless you also ship a migration path. The machinery is in
[`contracts/will/src/migration.rs`](./contracts/will/src/migration.rs):

- `CURRENT_SCHEMA_VERSION` is the version stamped on every newly created or
  migrated will.
- `decode_will` handles every read of a `DataKey::Will` entry. It tries the
  current layout first, then each legacy layout, and converts a legacy entry
  into the current `Will` shape. The converted will keeps its old
  `schema_version`.
- `upgrade` runs the `migrate_vN_to_vN+1` steps in order up to
  `CURRENT_SCHEMA_VERSION`. The owner-authorized `migrate_will` entry point
  calls it and persists the result.

**Version history**

| Version | Change |
|---|---|
| 0 | Wills written before `schema_version` was populated. |
| 1 | Layout with `keeper_bounty_bps` and `delegate`; no bounty-token selection. |
| 2 | Added `Will::keeper_bounty_token` (#109) and regrouped fields (#117). |
| 3 | Current layout. Added `Will::grace_extension_days` (#102), decoded from frozen prior-v2 entries with a zero default (#176). |

**To introduce version N+1** (for example, adding a field to `Will`):

1. Copy the current `Will` definition unchanged into a legacy type, e.g.
   `WillV1` in a `legacy` module, keeping `#[contracttype]` and the original
   field names. This is the only way old entries can still be decoded.
2. Change `Will` (add the field) and bump `CURRENT_SCHEMA_VERSION` to N+1.
3. In `decode_will`, add a fallback that tries `WillVN::try_from_val` and
   converts it into `Will`. Give new fields a safe default and leave
   `schema_version` at N.
4. Add a `migrate_vN_to_vN1` step and wire it into the `match` in `upgrade`.
   Every version below `CURRENT_SCHEMA_VERSION` needs an arm.
5. Add a row to the version history table above and a CHANGELOG entry.
6. Add a test that writes a `WillVN` directly under `DataKey::Will(id)`,
   loads it through `get_will`, and calls `migrate_will` to confirm it ends up
   at N+1 with the new field set.

Never reorder, rename or retype an existing field without going through these
steps. Entry points must also accept wills that have not been migrated yet,
since `migrate_will` is opt-in.

## Before opening a PR

<a id="before-opening-a-pr"></a>

Run every command used by the [Test CI workflow](./.github/workflows/test.yml) and confirm it succeeds:

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] `cargo build --workspace --release --target wasm32v1-none`
- [ ] `./.github/scripts/check-contract-version.sh`
- [ ] Confirm the Test workflow is green on the PR.

## Local setup

<a id="local-setup"></a>

See the [README](./README.md#local-setup) for toolchain installation and how to run the test suite.

## Task runner (just)

This repo ships a [`justfile`](./justfile) with shortcuts for the commands
above, so you don't have to remember or copy them by hand. Install
[`just`](https://github.com/casey/just#installation), then:

```bash
just --list    # see every available recipe
just lint      # cargo clippy --all-targets -- -D warnings
just test      # cargo test --workspace
just build     # cargo build --workspace --release --target wasm32v1-none
just fmt       # cargo fmt --all
just ci        # runs lint, test, and build in order — the full "Before opening a PR" checklist above in one command
```

If you don't have `just` installed, the raw `cargo` commands in
[Before opening a PR](#before-opening-a-pr) work identically.

## Cargo.lock policy

`Cargo.lock` is committed at the workspace root, and that's intentional:
every crate in this workspace is built and deployed as a Soroban
**contract** (compiled to a `wasm32v1-none` binary and deployed on-chain),
not published as a library for other crates to depend on via crates.io.
Committing the lockfile gives reproducible CI builds and deployments — the
same dependency graph every time, regardless of what's newest on crates.io
the day CI happens to run.

This does **not** constrain anyone who depends on a crate from this
workspace (e.g. `contracts/will`) as a path or git dependency in their own
project: Cargo resolves and locks dependencies per top-level workspace, so a
downstream consumer's own `Cargo.lock` governs their build, not this one.
Our committed lockfile only pins builds performed *inside this repository*
(CI, local `cargo test`/`cargo build`, `scripts/export-spec.sh`, etc.).

If this workspace ever adds a crate meant to be published to crates.io as a
reusable library (as opposed to an on-chain contract), revisit this policy
for that crate specifically — published library crates conventionally do
**not** commit `Cargo.lock`, so their consumers can resolve compatible
dependency versions themselves rather than inheriting exact pins.

## Mutation testing (cargo-mutants)

Code coverage alone can't tell you whether an assertion is too weak or an
edge case is missing — it only tells you a line ran, not that a test would
notice if the logic on that line broke. [`cargo-mutants`](https://mutants.rs/)
closes that gap: it systematically rewrites small pieces of the contract
(flipping a comparison, changing a constant, swapping a boolean) and reruns
the test suite against each mutant. A mutant that **survives** (tests still
pass) means no test would have caught that bug.

There is currently no CI workflow for mutation testing: it is a manual,
advisory check you run locally (see below). A survived mutant is a signal to
add a test, not a build failure.

### Running it locally

```sh
cargo install cargo-mutants --locked
cargo mutants --package will
```

This takes a while: cargo-mutants recompiles and reruns the test suite once
per candidate mutation. To scope a run while iterating on a single file:

```sh
cargo mutants --package will --file contracts/will/src/storage.rs
```

Results are written to `mutants.out/` (or wherever `--output` points).
Treat the report as advisory: review each surviving mutant and decide
whether it reveals a missing test or is an equivalent mutant that no test
can distinguish. Do not commit `mutants.out/`; it is a local artifact.

## Updating `deployments/testnet.json` after a redeploy

`deployments/testnet.json` records the contract IDs, Wasm hashes, and
metadata for the current testnet deployment. After a redeploy, update it in
the same PR as the deployment so the record never drifts from what is
actually on-chain.

1. Deploy the new Wasm and note the resulting contract ID and the Wasm hash
   reported by the deploy tooling.
2. Update the corresponding fields in `deployments/testnet.json` by hand.
   Do **not** blindly copy generated output over the file — generated output
   can drop fields, reorder keys, or carry stale values from a previous run.
3. Review the diff locally before committing:
   - Confirm the contract ID and Wasm hash match the deployment you just
     performed.
   - Confirm no unrelated entries changed.
   - Confirm the file is still valid JSON and that any schema fields the
     tooling expects are present.
4. Run the repository checks that consume the file (for example
   `./.github/scripts/check-contract-version.sh`) and confirm they pass.
5. Commit the reviewed record with a message that names the redeploy, and
   reference the deployment in the PR description.

If a field's meaning is unclear, stop and ask a maintainer rather than
guessing — an incorrect deployment record is worse than a missing one.

## Handling a flagged security advisory

`audit.toml` configures `cargo-audit` for this repository. When the scanner
flags an advisory, triage it before touching the ignore list:

1. **Understand the advisory.** Read the advisory entry (ID, affected
   crate, affected versions, patched versions) and determine whether the
   vulnerable code path is reachable from this workspace.
2. **Prefer a fix.** If a patched version exists and can be adopted, upgrade
   the dependency instead of ignoring the advisory.
3. **Justify any exception.** If the advisory cannot be fixed now (for
   example, no patched release exists, or the vulnerable path is provably
   unreachable), you may add it to the ignore list in `audit.toml` only
   together with:
   - a written justification comment next to the entry explaining why the
     advisory does not affect this repository and what would make it
     applicable again, and
   - a tracking issue (referenced in the comment) to remove the exception
     once a fix is available.
4. **Review locally.** Run the audit locally and confirm the remaining
   output is understood before committing.

Unjustified exceptions are not accepted. An ignore entry without a
justification comment and a tracking issue will be rejected in review.
