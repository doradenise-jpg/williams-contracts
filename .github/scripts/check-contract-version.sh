#!/bin/bash
set -euo pipefail

# check-contract-version.sh — verify that CONTRACT_VERSION (in
# contracts/will/src/lib.rs), the crate `version` in
# contracts/will/Cargo.toml, and (when HEAD is tagged) the git version tag all
# agree on the same semantic version (#501).
#
# This only checks *source-level* consistency, i.e. what will be true of the
# next build -- it cannot tell you whether a specific already-deployed
# contract instance is running a binary that matches this source tree. That
# is a separate, later check: scripts/deploy-testnet.sh calls the freshly
# deployed contract's `get_contract_version` right after deployment and
# compares it against CONTRACT_VERSION, because a source-only check can never
# catch a stale or wrong Wasm artifact being deployed.

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

LIB_RS="contracts/will/src/lib.rs"
CARGO_TOML="contracts/will/Cargo.toml"

# --- Extract CONTRACT_VERSION from lib.rs and decode it to semver ----------

raw_version_line="$(grep -E '^pub const CONTRACT_VERSION: u32 = ' "$LIB_RS" || true)"
if [[ -z "$raw_version_line" ]]; then
  echo "::error::could not find 'pub const CONTRACT_VERSION: u32 = ...' in $LIB_RS"
  exit 1
fi

# Pull out the numeric literal, stripping `_` digit separators (e.g. 1_002_000).
contract_version_raw="$(echo "$raw_version_line" | sed -E 's/^pub const CONTRACT_VERSION: u32 = ([0-9_]+);.*/\1/')"
contract_version="${contract_version_raw//_/}"

if ! [[ "$contract_version" =~ ^[0-9]+$ ]]; then
  echo "::error::failed to parse CONTRACT_VERSION as an integer from: $raw_version_line"
  exit 1
fi

major=$((contract_version / 1000000))
minor=$(((contract_version / 1000) % 1000))
patch=$((contract_version % 1000))
decoded_version="${major}.${minor}.${patch}"

# --- Extract the crate version from Cargo.toml ------------------------------

cargo_version="$(grep -m1 -E '^version = "' "$CARGO_TOML" | sed -E 's/^version = "(.*)"$/\1/')"

if [[ -z "$cargo_version" ]]; then
  echo "::error::could not find 'version = \"...\"' in $CARGO_TOML"
  exit 1
fi

echo "CONTRACT_VERSION ($LIB_RS):    $contract_version -> $decoded_version"
echo "crate version    ($CARGO_TOML): $cargo_version"

status=0

if [[ "$decoded_version" != "$cargo_version" ]]; then
  echo "::error::CONTRACT_VERSION decodes to '$decoded_version' but $CARGO_TOML has version '$cargo_version' -- bump both together (see CONTRIBUTING.md#contract-versioning)"
  status=1
fi

# --- If HEAD carries a release tag, it must match too ----------------------
#
# Only tags following the repository release-tag convention (an optional `v`
# prefix followed by a strict `MAJOR.MINOR.PATCH` semantic version) are treated
# as version assertions (#262). Ordinary nonrelease annotations (e.g.
# `deploy-testnet-2024-01-01`, `nightly`) are ignored so they cannot
# accidentally block local deployment/version validation.
#
# If several matching release tags point at HEAD, the selection is
# deterministic: the tags are sorted and the highest semantic version is
# chosen. If the matching tags disagree on the version, the check fails with a
# clear message rather than silently picking one.

release_tag_re='^v?[0-9]+\.[0-9]+\.[0-9]+$'

head_tags="$(git -C "$repo_root" tag --points-at HEAD 2>/dev/null || true)"
release_tags=()
while IFS= read -r candidate_tag; do
  [[ -z "$candidate_tag" ]] && continue
  if [[ "$candidate_tag" =~ $release_tag_re ]]; then
    release_tags+=("$candidate_tag")
  fi
done <<< "$head_tags"

if [[ "${#release_tags[@]}" -eq 0 ]]; then
  if [[ -n "$head_tags" ]]; then
    echo "git tag:                        (HEAD has no release tag, skipping tag check)"
  else
    echo "git tag:                        (HEAD is not tagged, skipping tag check)"
  fi
else
  # Deterministic ordering: sort by semantic version (numeric fields), then
  # by tag name as a tie-breaker, and select the highest.
  sorted_release_tags="$(printf '%s\n' "${release_tags[@]}" | sort -t. -k1,1V -k2,2n -k3,3n -k4,4)"
  selected_tag="$(printf '%s\n' "$sorted_release_tags" | tail -n 1)"
  selected_version="${selected_tag#v}"

  # Every matching release tag must agree on the version.
  tag_versions=()
  for candidate_tag in "${release_tags[@]}"; do
    tag_versions+=("${candidate_tag#v}")
  done
  unique_versions="$(printf '%s\n' "${tag_versions[@]}" | sort -u)"

  echo "git tag:                        $selected_tag -> $selected_version"
  if [[ "${#release_tags[@]}" -gt 1 ]]; then
    echo "git tags on HEAD:               ${release_tags[*]}"
  fi

  if [[ "$(printf '%s\n' "$unique_versions" | wc -l)" -gt 1 ]]; then
    echo "::error::multiple release tags on HEAD disagree on the version: ${release_tags[*]}"
    status=1
  elif [[ "$selected_version" != "$decoded_version" ]]; then
    echo "::error::git tag '$selected_tag' does not match CONTRACT_VERSION's decoded form '$decoded_version'"
    status=1
  fi
fi

if [[ "$status" -eq 0 ]]; then
  echo "OK: CONTRACT_VERSION, Cargo.toml, and any git tag agree on $decoded_version"
fi

exit "$status"
