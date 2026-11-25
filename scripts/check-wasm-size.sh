#!/usr/bin/env bash
# Verifies that the release WASM binary compiles and stays within
# the Soroban deployment size budget (docs/WASM_SIZE.md).
#
# Usage:
#   scripts/check-wasm-size.sh
#
# Environment variables:
#   MAX_WASM_SIZE_BYTES  Max allowed size in bytes (default: 250000)

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

# Preflight: fail early with an actionable diagnostic when a required
# executable is missing, so an unmet local prerequisite is distinguishable
# from a repository/build regression.
missing_tools=()
for tool in cargo stat; do
  if ! command -v "${tool}" >/dev/null 2>&1; then
    missing_tools+=("${tool}")
  fi
done

if [[ ${#missing_tools[@]} -gt 0 ]]; then
  for tool in "${missing_tools[@]}"; do
    echo "ERROR: required executable '${tool}' not found in PATH." >&2
  done
  echo "Install the missing tool(s) and re-run scripts/check-wasm-size.sh." >&2
  exit 127
fi

# Default budget if not supplied
MAX_WASM_SIZE_BYTES="${MAX_WASM_SIZE_BYTES:-250000}"

# Validate that the budget is a positive integer before proceeding
if ! [[ "$MAX_WASM_SIZE_BYTES" =~ ^[0-9]+$ ]] || [[ "$MAX_WASM_SIZE_BYTES" -le 0 ]]; then
  echo "ERROR: MAX_WASM_SIZE_BYTES must be a positive integer (got '${MAX_WASM_SIZE_BYTES}')." >&2
  exit 1
fi

wasm_path="target/wasm32v1-none/release/will.wasm"

echo "Building will contract (release, wasm32v1-none, locked)..."
"${repo_root}/scripts/build-wasm.sh" will

if [[ ! -f "${wasm_path}" ]]; then
  echo "ERROR: ${wasm_path} does not exist after build." >&2
  exit 1
fi

if stat --version >/dev/null 2>&1; then
  # GNU stat
  wasm_size="$(stat -c%s "${wasm_path}")"
else
  # BSD / macOS stat
  wasm_size="$(stat -f%z "${wasm_path}")"
fi

echo "Compiled WASM binary size: ${wasm_size} bytes (budget: ${MAX_WASM_SIZE_BYTES} bytes)"

if [[ "${wasm_size}" -gt "${MAX_WASM_SIZE_BYTES}" ]]; then
  echo "ERROR: ${wasm_path} size (${wasm_size} bytes) exceeds budget of ${MAX_WASM_SIZE_BYTES} bytes!" >&2
  exit 1
fi

echo "OK: WASM binary size is within budget (${wasm_size} <= ${MAX_WASM_SIZE_BYTES} bytes)."