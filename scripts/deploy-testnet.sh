#!/usr/bin/env bash
#
# deploy-testnet.sh — build and deploy the `will` contract to Stellar
# Testnet, then record the result in deployments/testnet.json.
#
# Prerequisites:
#   - stellar-cli >= 22.0.0 (`cargo install --locked stellar-cli`)
#   - rustup target wasm32v1-none (`rustup target add wasm32v1-none`)
#   - A funded testnet identity already configured in stellar-cli, e.g.:
#       stellar keys generate deployer --network testnet --fund
#     or, to import an existing key:
#       stellar keys add deployer --secret-key
#
# Configuration (environment variables):
#   DEPLOY_IDENTITY   Required. stellar-cli identity name used to sign and
#                      pay for the deployment. Must already exist and be
#                      funded on testnet.
#   NETWORK            Optional. Soroban network alias. Default: testnet
#                      Must be one of: testnet, futurenet, mainnet, sandbox.
#   RPC_URL             Optional. Soroban RPC endpoint. Must be consistent
#                      with NETWORK (see NETWORK_RPC_DEFAULTS below).
#                        Default: https://soroban-testnet.stellar.org
#   OUTPUT_FILE         Optional. Deployment artifact path. Defaults to
#                      deployments/<NETWORK>.json, so a non-testnet run can
#                      never overwrite deployments/testnet.json.
#
# Usage:
#   DEPLOY_IDENTITY=deployer ./scripts/deploy-testnet.sh
#
# After it finishes, review and commit the updated deployment artifact
# (deployments/testnet.json for the default network)
# — see CONTRIBUTING.md#updating-deploymentstestnetjson-after-a-redeploy.
#
# Version consistency (#501): CONTRACT_VERSION being correct in source is not
# the same thing as the *deployed* contract actually running that source. A
# stale local build, a wrong --wasm path, or deploying from the wrong git
# worktree would all still "succeed" as far as `stellar contract deploy` is
# concerned. So after deploying, this script calls the freshly deployed
# contract's own `get_contract_version` over RPC and fails loudly, before
# writing deployments/testnet.json, if it disagrees with the source
# CONTRACT_VERSION that was supposedly just built and deployed.
#
# Deployment artifact (#87): deployments/testnet.json records the concrete
# contract ID, the network, an ISO-8601 deployment timestamp, and the SHA-256
# hash of the deployed WASM bytecode so the artifact can be verified against
# the current release build.
#
# Provenance (#285): the artifact also records a nested `provenance` object
# identifying the source revision (git HEAD), whether the worktree was dirty,
# the pinned tool identifiers (rustc/cargo/stellar-cli versions and the
# wasm32v1-none target) and the canonical build invocation. A hash alone does
# not tell a reviewer which source state produced it; this record does. Dirty
# builds are explicitly marked (dirty: true plus a dirtyDisclosure string)
# rather than being presented as the clean revision. No secrets, private keys
# or environment variables beyond the already-recorded identity name are
# serialized.
#
# Testing: `./scripts/deploy-testnet.sh --emit-provenance-only` writes just the
# provenance record (no build, no deploy) so tests can exercise it in clean and
# dirty temporary git trees.

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

: "${DEPLOY_IDENTITY:?Set DEPLOY_IDENTITY to a funded stellar-cli identity name, e.g. DEPLOY_IDENTITY=deployer ./scripts/deploy-testnet.sh}"
NETWORK="${NETWORK:-testnet}"

# Known networks and their canonical RPC endpoints. Keeping this mapping in
# one place lets us reject unknown networks and catch a NETWORK/RPC_URL
# mismatch before any transaction is submitted.
declare -A NETWORK_RPC_DEFAULTS=(
  [testnet]="https://soroban-testnet.stellar.org"
  [futurenet]="https://rpc-futurenet.stellar.org"
  [mainnet]="https://soroban-rpc.mainnet.stellar.gateway.fm"
  [sandbox]="http://localhost:8000/soroban/rpc"
)

if [[ -z "${NETWORK_RPC_DEFAULTS[$NETWORK]+set}" ]]; then
  echo "error: unsupported NETWORK '$NETWORK'. Supported networks: ${!NETWORK_RPC_DEFAULTS[*]}" >&2
  exit 1
fi

RPC_URL="${RPC_URL:-${NETWORK_RPC_DEFAULTS[$NETWORK]}}"
WASM_PATH="target/wasm32v1-none/release/will.wasm"
# Network-specific artifact path: a non-testnet run writes its own file and
# can never silently overwrite deployments/testnet.json.
OUTPUT_FILE="${OUTPUT_FILE:-deployments/${NETWORK}.json}"
LIB_RS="contracts/will/src/lib.rs"
# The canonical build invocation. Recorded verbatim in the provenance record so
# a reviewer can reproduce the artifact under the same pinned settings.
BUILD_COMMAND="cargo build --package will --release --target wasm32v1-none"
WASM_TARGET="wasm32v1-none"

# --emit-provenance-only: write just the provenance record (no build, no
# deploy) so tests can generate it in clean/dirty temporary git trees.
EMIT_PROVENANCE_ONLY=0
for arg in "$@"; do
  case "$arg" in
    --emit-provenance-only) EMIT_PROVENANCE_ONLY=1 ;;
    *)
      echo "error: unknown argument '$arg'" >&2
      exit 1
      ;;
  esac
done

# JSON-escape a string for safe embedding in the artifact. Only quotes and
# backslashes need escaping for the values we record (versions, paths, commit
# hashes); control characters are stripped defensively.
json_escape() {
  local s="$1"
  s="${s//\\/\\\\}"
  s="${s//\"/\\\"}"
  s="${s//$'\n'/ }"
  s="${s//$'\r'/ }"
  s="${s//$'\t'/ }"
  printf '%s' "$s"
}

# Capture the source revision and dirty state. Fails loudly if this is not a
# git worktree, since provenance without a revision is meaningless.
capture_git_provenance() {
  if ! command -v git >/dev/null 2>&1; then
    echo "error: git not found on PATH; cannot record source provenance" >&2
    exit 1
  fi
  if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    echo "error: not inside a git worktree; cannot record source provenance" >&2
    exit 1
  fi
  GIT_REVISION="$(git rev-parse HEAD)"
  if [[ -n "$(git status --porcelain)" ]]; then
    GIT_DIRTY=true
    GIT_DIRTY_DISCLOSURE="worktree had uncommitted changes at build time; this artifact does NOT correspond to clean revision ${GIT_REVISION}"
    echo "warning: git worktree is dirty -- the recorded revision ${GIT_REVISION} does not fully describe the built source" >&2
  else
    GIT_DIRTY=false
    GIT_DIRTY_DISCLOSURE=""
  fi
}

# Capture pinned tool identifiers. Missing tools are recorded as "unknown"
# rather than aborting, so the record is still emitted for review.
capture_tool_provenance() {
  if command -v rustc >/dev/null 2>&1; then
    RUSTC_VERSION="$(rustc --version)"
  else
    RUSTC_VERSION="unknown"
  fi
  if command -v cargo >/dev/null 2>&1; then
    CARGO_VERSION="$(cargo --version)"
  else
    CARGO_VERSION="unknown"
  fi
  if command -v stellar >/dev/null 2>&1; then
    STELLAR_CLI_VERSION="$(stellar --version 2>/dev/null | head -n1)"
  else
    STELLAR_CLI_VERSION="unknown"
  fi
}

# Emit the provenance JSON object (without surrounding braces) to stdout.
emit_provenance_fields() {
  local wasm_hash="${1:-}"
  printf '    "revision": "%s",\n' "$(json_escape "$GIT_REVISION")"
  printf '    "dirty": %s,\n' "$GIT_DIRTY"
  printf '    "dirtyDisclosure": "%s",\n' "$(json_escape "$GIT_DIRTY_DISCLOSURE")"
  printf '    "rustcVersion": "%s",\n' "$(json_escape "$RUSTC_VERSION")"
  printf '    "cargoVersion": "%s",\n' "$(json_escape "$CARGO_VERSION")"
  printf '    "stellarCliVersion": "%s",\n' "$(json_escape "$STELLAR_CLI_VERSION")"
  printf '    "wasmTarget": "%s",\n' "$(json_escape "$WASM_TARGET")"
  printf '    "buildCommand": "%s",\n' "$(json_escape "$BUILD_COMMAND")"
  printf '    "wasmHash": "%s"\n' "$(json_escape "$wasm_hash")"
}

if [[ "$EMIT_PROVENANCE_ONLY" == "1" ]]; then
  capture_git_provenance
  capture_tool_provenance
  mkdir -p "$(dirname "$OUTPUT_FILE")"
  {
    printf '{\n'
    printf '  "provenance": {\n'
    emit_provenance_fields "${WASM_HASH:-}"
    printf '  }\n'
    printf '}\n'
  } > "$OUTPUT_FILE"
  echo "==> Wrote provenance-only record to $OUTPUT_FILE"
  exit 0
fi

# Guard against an explicit OUTPUT_FILE that would clobber the testnet record
# from a non-testnet run.
if [[ "$NETWORK" != "testnet" && "$OUTPUT_FILE" == "deployments/testnet.json" ]]; then
  echo "error: refusing to write a '$NETWORK' deployment to $OUTPUT_FILE (the testnet record)." >&2
  echo "error: unset OUTPUT_FILE to use the default deployments/${NETWORK}.json." >&2
  exit 1
fi

command -v stellar >/dev/null 2>&1 || {
  echo "error: stellar-cli not found on PATH. Install it with:" >&2
  echo "  cargo install --locked stellar-cli" >&2
  exit 1
}

echo "==> Capturing source and build provenance"
capture_git_provenance
capture_tool_provenance

echo "==> Checking CONTRACT_VERSION is consistent with Cargo.toml before building"
"$ROOT_DIR/.github/scripts/check-contract-version.sh"

echo "==> Building contract for wasm32v1-none (release, locked, canonical flags)"
"$ROOT_DIR/scripts/build-wasm.sh" --package will

if [[ ! -f "$WASM_PATH" ]]; then
  echo "error: expected wasm artifact at $WASM_PATH, but it was not produced" >&2
  exit 1
fi

echo "==> Computing SHA-256 hash of deployed WASM bytecode"
if command -v sha256sum >/dev/null 2>&1; then
  WASM_HASH="$(sha256sum "$WASM_PATH" | awk '{print $1}')"
else
  WASM_HASH="$(shasum -a 256 "$WASM_PATH" | awk '{print $1}')"
fi
if [[ -z "$WASM_HASH" ]]; then
  echo "error: failed to compute SHA-256 hash of $WASM_PATH" >&2
  exit 1
fi

echo "==> Deploying to '$NETWORK' as identity '$DEPLOY_IDENTITY'"
CONTRACT_ID="$(stellar contract deploy \
  --wasm "$WASM_PATH" \
  --source "$DEPLOY_IDENTITY" \
  --network "$NETWORK" \
  --rpc-url "$RPC_URL")"

if [[ -z "$CONTRACT_ID" ]]; then
  echo "error: stellar contract deploy did not return a contract id" >&2
  exit 1
fi

DEPLOYED_AT="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

echo "==> Deployed WillContract: $CONTRACT_ID at $DEPLOYED_AT (wasm sha256: $WASM_HASH)"

echo "==> Verifying deployed contract's get_contract_version matches source CONTRACT_VERSION"
EXPECTED_VERSION_RAW="$(grep -E '^pub const CONTRACT_VERSION: u32 = ' "$LIB_RS" | sed -E 's/^pub const CONTRACT_VERSION: u32 = ([0-9_]+);.*/\1/')"
EXPECTED_VERSION="${EXPECTED_VERSION_RAW//_/}"

ONCHAIN_VERSION_RAW="$(stellar contract invoke \
  --id "$CONTRACT_ID" \
  --source "$DEPLOY_IDENTITY" \
  --network "$NETWORK" \
  --rpc-url "$RPC_URL" \
  -- get_contract_version)"
# stellar-cli prints scalar results as bare JSON (a plain number here); strip
# any surrounding quotes/whitespace defensively before comparing.
ONCHAIN_VERSION="$(echo "$ONCHAIN_VERSION_RAW" | tr -d '"[:space:]')"

if [[ "$ONCHAIN_VERSION" != "$EXPECTED_VERSION" ]]; then
  echo "error: deployed contract $CONTRACT_ID reports get_contract_version() = $ONCHAIN_VERSION, but source CONTRACT_VERSION is $EXPECTED_VERSION" >&2
  echo "error: the deployment is inconsistent with the source tree -- do NOT record this in $OUTPUT_FILE. Check for a stale build or wrong --wasm artifact." >&2
  exit 1
fi
echo "==> OK: on-chain version $ONCHAIN_VERSION matches source CONTRACT_VERSION"

echo "==> Verifying deployed bytecode hash matches local WASM SHA-256"
# `stellar contract fetch` retrieves the deployed WASM bytecode for the
# contract id; hashing it lets us assert the on-chain instance matches the
# audited local build artifact before recording the deployment.
FETCHED_WASM="$(mktemp)"
trap 'rm -f "$FETCHED_WASM"' EXIT
stellar contract fetch \
  --id "$CONTRACT_ID" \
  --network "$NETWORK" \
  --rpc-url "$RPC_URL" \
  --out-file "$FETCHED_WASM"
if command -v sha256sum >/dev/null 2>&1; then
  DEPLOYED_WASM_HASH="$(sha256sum "$FETCHED_WASM" | awk '{print $1}')"
else
  DEPLOYED_WASM_HASH="$(shasum -a 256 "$FETCHED_WASM" | awk '{print $1}')"
fi
echo "==> Deployed WASM SHA-256: $DEPLOYED_WASM_HASH"

if [[ "$DEPLOYED_WASM_HASH" != "$WASM_HASH" ]]; then
  echo "error: deployed bytecode hash $DEPLOYED_WASM_HASH does not match local WASM hash $WASM_HASH" >&2
  echo "error: the deployed contract does not match the audited local build -- do NOT record this in $OUTPUT_FILE." >&2
  exit 1
fi
echo "==> OK: deployed bytecode hash matches local WASM SHA-256"

mkdir -p "$(dirname "$OUTPUT_FILE")"
{
  printf '{\n'
  printf '  "WillContract": "%s",\n' "$(json_escape "$CONTRACT_ID")"
  printf '  "network": "%s",\n' "$(json_escape "$NETWORK")"
  printf '  "rpcUrl": "%s",\n' "$(json_escape "$RPC_URL")"
  printf '  "identity": "%s",\n' "$(json_escape "$DEPLOY_IDENTITY")"
  printf '  "deployedAt": "%s",\n' "$(json_escape "$DEPLOYED_AT")"
  printf '  "wasmHash": "%s",\n' "$(json_escape "$WASM_HASH")"
  printf '  "provenance": {\n'
  emit_provenance_fields "$WASM_HASH"
  printf '  }\n'
  printf '}\n'
} > "$OUTPUT_FILE"

echo "==> Wrote $OUTPUT_FILE"
echo
echo "Next steps:"
echo "  1. Review the diff: git diff -- $OUTPUT_FILE"
echo "  2. Commit it on its own: git add $OUTPUT_FILE && git commit -m 'chore: record $NETWORK deployment $CONTRACT_ID'"
echo "  3. See CONTRIBUTING.md#updating-deploymentstestnetjson-after-a-redeploy for the full checklist."
