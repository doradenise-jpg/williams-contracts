#!/usr/bin/env bash
# Export the compiled `will` contract spec to spec/will-v<version>.json.
#
# Usage:
#   scripts/export-spec.sh                        write spec/will-v<version>.json
#   scripts/export-spec.sh --check                verify the committed snapshot is current
#   scripts/export-spec.sh --update-unreleased    replace an existing snapshot for an
#                                                 unreleased version
#
# Only zero arguments, exactly one --check, or exactly one --update-unreleased
# are accepted; any other combination is a usage error (exit 2).
#
# Published snapshots are never overwritten by ordinary generation: if
# spec/will-v<version>.json already exists, the write mode refuses to replace
# it unless --update-unreleased is passed. That flag is only for regenerating a
# snapshot whose version has not been released yet. The export is written to a
# temporary file first and only moved into place on success, so a failed export
# leaves any existing snapshot bytes untouched.
#
# --check exports to a temporary file and compares it against the committed
# snapshot. It exits non-zero when they differ, or when no snapshot exists for
# the crate version currently declared — the case where a public signature
# changed and the version was bumped but the export step was skipped.
#
# Requires: cargo, the wasm32v1-none target, stellar-cli >= 22.0.0 (`cargo install
# --locked stellar-cli`), and jq.
#
# STELLAR_BIN overrides the exporter binary (used by the tests).
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

usage() {
  echo "Usage: scripts/export-spec.sh [--check | --update-unreleased]" >&2
}

check_mode=0
update_mode=0
if [[ "$#" -eq 0 ]]; then
  :
elif [[ "$#" -eq 1 && "$1" == "--check" ]]; then
  check_mode=1
elif [[ "$#" -eq 1 && "$1" == "--update-unreleased" ]]; then
  update_mode=1
else
  if [[ "$#" -gt 0 ]]; then
    echo "Unknown argument: $1" >&2
  fi
  usage
  exit 2
fi

STELLAR_BIN="${STELLAR_BIN:-stellar}"
WASM_PATH="target/wasm32v1-none/release/will.wasm"

crate_version() {
  cargo metadata --no-deps --format-version 1 \
    | jq -r '.packages[] | select(.name=="will") | .version'
}

# Export the embedded SCSpecEntry stream from the compiled wasm as JSON.
# Writes to $1. Propagates any exporter failure to the caller.
export_spec() {
  local out="$1"
  "$STELLAR_BIN" contract bindings json \
    --wasm "$WASM_PATH" \
    --output "$out"
}

echo "Building will contract (canonical locked release build)..."
"${ROOT_DIR}/scripts/build-wasm.sh" will

if [[ ! -f "$WASM_PATH" ]]; then
  echo "error: expected wasm artifact not found at $WASM_PATH" >&2
  exit 1
fi

version="$(crate_version)"
if [[ -z "$version" || "$version" == "null" ]]; then
  echo "error: could not determine will crate version" >&2
  exit 1
fi
out_path="spec/will-v${version}.json"

if [[ "${check_mode}" -eq 0 ]]; then
  if [[ -e "${out_path}" && "${update_mode}" -eq 0 ]]; then
    echo "ERROR: ${out_path} already exists." >&2
    echo "Published spec snapshots are never overwritten by ordinary generation." >&2
    echo "If version ${version} is not released yet, regenerate it with:" >&2
    echo "  scripts/export-spec.sh --update-unreleased" >&2
    echo "Otherwise bump the version in contracts/will/Cargo.toml and export again." >&2
    exit 1
  fi

  tmp_dir="$(mktemp -d)"
  trap 'rm -rf "${tmp_dir}"' EXIT
  fresh_path="${tmp_dir}/will-v${version}.json"

  echo "Exporting spec to ${out_path}..."
  export_spec "${fresh_path}"
  mv -f "${fresh_path}" "${out_path}"
  echo "Done. Diff ${out_path} against the previous version to review drift."
  exit 0
fi

mkdir -p spec
tmp_file="$(mktemp "spec/.will-v${version}.XXXXXX.json")"
# shellcheck disable=SC2064
trap "rm -f '$tmp_file'" EXIT

echo "Exporting spec to $out_path..."
if ! export_spec "$tmp_file"; then
  echo "error: spec export failed; snapshot left untouched" >&2
  exit 1
fi

if [[ ! -s "$tmp_file" ]]; then
  echo "error: exporter produced no output; snapshot left untouched" >&2
  exit 1
fi

if [[ ! -f "$out_path" ]]; then
  echo "error: missing spec snapshot $out_path" >&2
  exit 1
fi
if ! diff -u "$out_path" "$tmp_file"; then
  echo "error: $out_path is stale; run scripts/export-spec.sh" >&2
  exit 1
fi
echo "Spec snapshot $out_path is up to date."
