#!/usr/bin/env bash
#
# Regenerates the resource-cost profile (contracts/will/src/profile.rs) and
# fails if it no longer matches the "## Current profile" table committed in
# docs/RESOURCE_COSTS.md, so a change that meaningfully shifts an entry
# point's ledger footprint can't silently leave the published numbers stale
# (issue #268).
#
# The profile is a deterministic measurement against soroban-sdk's mock Env
# (no wall-clock timing, no randomness), so an exact diff is the right check
# here, not a fuzzy tolerance.
#
# If the profile module is disabled (mod profile; is commented out in lib.rs),
# the script fails by default so a skipped profiling run can never be mistaken
# for a verified match (issue #60, #241). Set ALLOW_SKIPPED_PROFILE_COSTS=1 to
# opt out explicitly (e.g. for local work on unrelated changes).
#
# Usage: scripts/check_resource_costs.sh
=======

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

# Allow tests to substitute the cargo binary (mirrors STELLAR_BIN in
# export-spec.sh) so the offline command-stub suite can exercise this script
# without a real toolchain.
cargo_bin="${CARGO_BIN:-cargo}"

# Detect whether the profile module is active in lib.rs.
# The line is considered disabled when it is commented out (leading // or #).
if ! grep -qE '^[[:space:]]*mod profile;' contracts/will/src/lib.rs; then
  if [ "${ALLOW_SKIPPED_PROFILE_COSTS:-0}" = "1" ]; then
    echo "INFO: The profile module (mod profile;) is currently disabled in"
    echo "      contracts/will/src/lib.rs. Resource-cost profiling is skipped"
    echo "      (ALLOW_SKIPPED_PROFILE_COSTS=1). Measurements were NOT verified."
    exit 0
  fi
  echo "ERROR: The profile module (mod profile;) is disabled in" >&2
  echo "       contracts/will/src/lib.rs, so resource costs cannot be verified." >&2
  echo "       Uncomment the 'mod profile;' line and re-run this script, or set" >&2
  echo "       ALLOW_SKIPPED_PROFILE_COSTS=1 to skip the check explicitly." >&2
  exit 1
fi

extract_table() {
  awk '/^\| entry point \|/{flag=1} flag && /^\|/{print} flag && !/^\|/{flag=0}'
}

# Preflight: the profile is produced by a Cargo test run, so Cargo must be
# present before any work happens. Report the missing executable explicitly so
# contributors can tell an unmet local prerequisite from a repository
# regression (issue #260).
if ! command -v cargo >/dev/null 2>&1; then
  echo "ERROR: required executable 'cargo' was not found on PATH." >&2
  echo "       Resource-cost profiling needs Cargo to run:" >&2
  echo "         cargo test -p will --lib profile -- --nocapture" >&2
  echo "       Install a Rust toolchain (https://rustup.rs) and re-run this script." >&2
  exit 127
fi

echo "Running the resource-cost profile..."
# Capture combined output without letting `set -e` abort the script from inside
# the command substitution: an installed-but-failing Cargo must surface its
# diagnostics (and original exit status) rather than exiting silently.
profile_status=0
profile_output="$("$cargo_bin" test -p will --lib profile -- --nocapture 2>&1)" || profile_status=$?

if [ "$profile_status" -ne 0 ]; then
  echo "ERROR: '$cargo_bin test -p will --lib profile -- --nocapture' failed with exit status ${profile_status}." >&2
  echo "       Captured output follows:" >&2
  printf '%s\n' "$profile_output" >&2
  exit "$profile_status"
fi

generated_table="$(printf '%s\n' "$profile_output" | extract_table)"
committed_table="$(extract_table < docs/RESOURCE_COSTS.md)"

if [ -z "$generated_table" ]; then
  echo "Could not find a profile table in the test output. Full output:" >&2
  printf '%s\n' "$profile_output" >&2
  exit 1
fi

if [ "$generated_table" != "$committed_table" ]; then
  echo "Resource costs have drifted from docs/RESOURCE_COSTS.md." >&2
  echo "Re-run 'cargo test -p will --lib profile -- --nocapture' and update" >&2
  echo "the '## Current profile' table with the new numbers." >&2
  echo >&2
  echo "--- committed ---" >&2
  printf '%s\n' "$committed_table" >&2
  echo "--- freshly measured ---" >&2
  printf '%s\n' "$generated_table" >&2
  exit 1
fi

echo "Resource costs match docs/RESOURCE_COSTS.md."
