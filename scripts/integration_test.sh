#!/usr/bin/env bash
# Integration smoke test for the will contract against a local standalone node.
#
# Exercises the full create -> check-in -> cancel lifecycle using the compiled
# WASM bindings and the installed Stellar CLI. Requires a running standalone
# node and a pinned Stellar CLI version.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

NETWORK="local"
WASM_PATH="target/wasm32v1-none/release/will.wasm"

# Isolate all stellar-cli state (keys, network config) for this run so we never
# touch the user's existing identities or network aliases, and so parallel runs
# do not interfere with each other.
CLI_CONFIG_DIR="$(mktemp -d "${TMPDIR:-/tmp}/will-integration.XXXXXX")"
export STELLAR_CONFIG_DIR="$CLI_CONFIG_DIR"
export HOME="$CLI_CONFIG_DIR"

# Unique per-run aliases so we never collide with (or overwrite) preexisting
# identities or network aliases.
RUN_SUFFIX="$$"
IDENTITY="integration-test-$RUN_SUFFIX"
BENEFICIARY="beneficiary-tmp-$RUN_SUFFIX"

# Ownership tracking: only clean up resources this run actually created.
network_started=0
identity_created=0
beneficiary_created=0

log() {
  printf '\n\033[1;34m[integration]\033[0m %s\n' "$1"
}

# Log a lifecycle stage boundary so a failing create/check-in/cancel step is
# identifiable in CI logs.
stage() {
  printf '\n\033[1;36m[integration][stage]\033[0m %s\n' "$1"
}

# Run a lifecycle invocation, reporting the stage name and exit status on
# failure. The script runs under `set -e`, so a nonzero status here aborts the
# whole run and propagates to the caller (e.g. the CI workflow job).
run_stage() {
  local name="$1"
  shift
  stage "BEGIN $name"
  if ! "$@"; then
    local status=$?
    printf '\033[1;31m[integration][stage]\033[0m FAILED %s (exit %d)\n' "$name" "$status" >&2
    exit "$status"
  fi
  stage "OK $name"
}

cleanup() {
  local status=$?
  if [ "$beneficiary_created" -eq 1 ]; then
    log "Removing test beneficiary identity $BENEFICIARY"
    stellar keys remove "$BENEFICIARY" >/dev/null 2>&1 || true
  fi
  if [ "$identity_created" -eq 1 ]; then
    log "Removing test identity $IDENTITY"
    stellar keys remove "$IDENTITY" >/dev/null 2>&1 || true
  fi
  if [ "$network_started" -eq 1 ]; then
    log "Stopping local Soroban network"
    stellar network stop "$NETWORK" >/dev/null 2>&1 || true
  fi
  if [ -n "${CLI_CONFIG_DIR:-}" ] && [ -d "$CLI_CONFIG_DIR" ]; then
    rm -rf "$CLI_CONFIG_DIR"
  fi
  exit "$status"
}
trap cleanup EXIT

log "Building contract for wasm32v1-none (release)"
cargo build --package will --release --target wasm32v1-none

if [ ! -f "$WASM_PATH" ]; then
  echo "Expected wasm artifact not found at $WASM_PATH" >&2
  exit 1
fi

log "Starting local Soroban standalone network"
stellar network start "$NETWORK" --limits testnet >/dev/null 2>&1 || \
  stellar network start "$NETWORK"
network_started=1

log "Generating a funded identity for the test run"
stellar keys generate "$IDENTITY" --network "$NETWORK" --fund
identity_created=1

OWNER_ADDR="$(stellar keys address "$IDENTITY")"

log "Deploying will.wasm to $NETWORK"
CONTRACT_ID="$(stellar contract deploy \
  --wasm "$WASM_PATH" \
  --source "$IDENTITY" \
  --network "$NETWORK" | tail -n 1)"
echo "Deployed contract id: $CONTRACT_ID"

log "Deploying a test SEP-41 token (native XLM stand-in) to fund the will"
TOKEN_ID="$(stellar contract asset deploy \
  --asset native \
  --source "$IDENTITY" \
  --network "$NETWORK" 2>/dev/null | tail -n 1 || echo "")"

if [ -z "$TOKEN_ID" ]; then
  echo "Could not resolve a native asset contract id; aborting" >&2
  exit 1
fi

stellar keys generate "$BENEFICIARY" --network "$NETWORK" --fund
beneficiary_created=1
BENEFICIARY_ADDR="$(stellar keys address "$BENEFICIARY")"

# ---------------------------------------------------------------------------
# Custody assertions
#
# The lifecycle below moves exactly DEPOSIT_AMOUNT stroops of the test token
# from the owner into the contract on create_will, and must move exactly that
# amount back on cancel_will. We read the token balances of both parties
# before and after each step and assert the exact deltas, so a build with
# broken transfers or a wrong refund amount fails the smoke test even if the
# will still reaches the expected status.
#
# Negative fixture: set INTEGRATION_EXPECTED_REFUND to a deliberately wrong
# value to verify the refund assertion actually fires, e.g.
#
#   INTEGRATION_EXPECTED_REFUND=999 ./scripts/integration_test.sh
#
# The script must then fail with a refund-mismatch error.
# ---------------------------------------------------------------------------
DEPOSIT_AMOUNT=1000
EXPECTED_REFUND="${INTEGRATION_EXPECTED_REFUND:-$DEPOSIT_AMOUNT}"

# token_balance <address>: prints the token balance of <address> as an integer.
token_balance() {
  local addr="$1"
  local raw
  raw="$(stellar contract invoke \
    --id "$TOKEN_ID" \
    --source "$IDENTITY" \
    --network "$NETWORK" \
    -- balance --id "$addr" 2>/dev/null | tail -n 1)"
  # The CLI may print the integer directly or wrap it in quotes/JSON.
  raw="${raw//\"/}"
  raw="${raw//[[:space:]]/}"
  if ! [[ "$raw" =~ ^[0-9]+$ ]]; then
    echo "Could not parse token balance for $addr (got: '$raw')" >&2
    return 1
  fi
  printf '%s\n' "$raw"
}

# assert_delta <label> <before> <after> <expected_delta>
assert_delta() {
  local label="$1" before="$2" after="$3" expected="$4"
  local actual=$(( after - before ))
  if [ "$actual" -ne "$expected" ]; then
    echo "Custody assertion failed ($label): expected delta $expected, got $actual (before=$before after=$after)" >&2
    exit 1
  fi
  echo "  $label: delta $actual (expected $expected) OK"
}

log "Capturing pre-deposit token balances"
OWNER_BALANCE_BEFORE="$(token_balance "$OWNER_ADDR")"
CONTRACT_BALANCE_BEFORE="$(token_balance "$CONTRACT_ID")"
echo "Owner balance before: $OWNER_BALANCE_BEFORE"
echo "Contract balance before: $CONTRACT_BALANCE_BEFORE"

log "create_will: locking $DEPOSIT_AMOUNT stroops for one beneficiary, 1-day check-in / 1-day grace"
stage "BEGIN create_will"
if ! WILL_ID="$(stellar contract invoke \
  --id "$CONTRACT_ID" \
  --source "$IDENTITY" \
  --network "$NETWORK" \
  -- create_will \
  --owner "$OWNER_ADDR" \
  --tokens "[[\"$TOKEN_ID\",\"1000\"]]" \
  --beneficiaries "[{\"address\":\"$BENEFICIARY_ADDR\",\"basis_points\":10000}]" \
  --checkin_period_days 1 \
  --grace_period_days 1 \
  --guardians "[]" \
  --guardian_threshold 0 \
  --keeper_bounty_bps "null" \
  --confirmation_delay_seconds 0)"; then
  status=$?
  printf '\033[1;31m[integration][stage]\033[0m FAILED create_will (exit %d)\n' "$status" >&2
  exit "$status"
fi
stage "OK create_will"
echo "Created will id: $WILL_ID"

log "Asserting deposit custody moved exactly $DEPOSIT_AMOUNT stroops into the contract"
OWNER_BALANCE_AFTER_DEPOSIT="$(token_balance "$OWNER_ADDR")"
CONTRACT_BALANCE_AFTER_DEPOSIT="$(token_balance "$CONTRACT_ID")"
assert_delta "owner balance after deposit" \
  "$OWNER_BALANCE_BEFORE" "$OWNER_BALANCE_AFTER_DEPOSIT" "-$DEPOSIT_AMOUNT"
assert_delta "contract balance after deposit" \
  "$CONTRACT_BALANCE_BEFORE" "$CONTRACT_BALANCE_AFTER_DEPOSIT" "$DEPOSIT_AMOUNT"

log "get_will: reading back the will we just created"
WILL_AFTER_DEPOSIT="$(stellar contract invoke \
  --id "$CONTRACT_ID" \
  --source "$IDENTITY" \
  --network "$NETWORK" \
  -- get_will --will_id "$WILL_ID")"
echo "$WILL_AFTER_DEPOSIT"

# The will must record the deposited amount, matching the observed token delta.
if [[ "$WILL_AFTER_DEPOSIT" != *"$DEPOSIT_AMOUNT"* ]]; then
  echo "Will balance after deposit does not record $DEPOSIT_AMOUNT: $WILL_AFTER_DEPOSIT" >&2
  exit 1
fi
echo "  will balance after deposit agrees with token delta OK"

log "get_will_status: expect Active"
STATUS="$(stellar contract invoke \
  --id "$CONTRACT_ID" \
  --source "$IDENTITY" \
  --network "$NETWORK" \
  -- get_will_status --will_id "$WILL_ID")"
echo "Status: $STATUS"
if [[ "$STATUS" != *"Active"* ]]; then
  echo "Expected Active status after create_will, got: $STATUS" >&2
  exit 1
fi

log "check_in: resetting the deadline"
run_stage check_in \
  stellar contract invoke \
  --id "$CONTRACT_ID" \
  --source "$IDENTITY" \
  --network "$NETWORK" \
  -- check_in --will_id "$WILL_ID" --owner "$OWNER_ADDR"

log "cancel_will: withdrawing the balance and closing the will"
run_stage cancel_will \
  stellar contract invoke \
  --id "$CONTRACT_ID" \
  --source "$IDENTITY" \
  --network "$NETWORK" \
  -- cancel_will --will_id "$WILL_ID" --owner "$OWNER_ADDR"

log "Asserting refund custody returned exactly $EXPECTED_REFUND stroops to the owner"
OWNER_BALANCE_AFTER_REFUND="$(token_balance "$OWNER_ADDR")"
CONTRACT_BALANCE_AFTER_REFUND="$(token_balance "$CONTRACT_ID")"
assert_delta "owner balance after refund" \
  "$OWNER_BALANCE_AFTER_DEPOSIT" "$OWNER_BALANCE_AFTER_REFUND" "$EXPECTED_REFUND"
assert_delta "contract balance after refund" \
  "$CONTRACT_BALANCE_AFTER_DEPOSIT" "$CONTRACT_BALANCE_AFTER_REFUND" "-$EXPECTED_REFUND"

# The contract must be back to its pre-deposit balance (no stranded funds).
if [ "$CONTRACT_BALANCE_AFTER_REFUND" -ne "$CONTRACT_BALANCE_BEFORE" ]; then
  echo "Contract balance after refund ($CONTRACT_BALANCE_AFTER_REFUND) does not match pre-deposit balance ($CONTRACT_BALANCE_BEFORE)" >&2
  exit 1
fi
echo "  contract balance restored to pre-deposit value OK"

log "get_will: reading back the will after cancellation"
WILL_AFTER_CANCEL="$(stellar contract invoke \
  --id "$CONTRACT_ID" \
  --source "$IDENTITY" \
  --network "$NETWORK" \
  -- get_will --will_id "$WILL_ID")"
echo "$WILL_AFTER_CANCEL"

# The will must no longer record the deposited balance after the refund.
if [[ "$WILL_AFTER_CANCEL" == *"$DEPOSIT_AMOUNT"* ]]; then
  echo "Will balance after cancel still records $DEPOSIT_AMOUNT: $WILL_AFTER_CANCEL" >&2
  exit 1
fi
echo "  will balance after cancel agrees with token delta OK"

log "get_will_status: expect Cancelled"
FINAL_STATUS="$(stellar contract invoke \
  --id "$CONTRACT_ID" \
  --source "$IDENTITY" \
  --network "$NETWORK" \
  -- get_will_status --will_id "$WILL_ID")"
echo "Status: $FINAL_STATUS"
if [[ "$FINAL_STATUS" != *"Cancelled"* ]]; then
  echo "Expected Cancelled status after cancel_will, got: $FINAL_STATUS" >&2
  exit 1
fi

log "All lifecycle invocations against the real wasm artifact succeeded."
log "Custody verified: $DEPOSIT_AMOUNT stroops deposited and $EXPECTED_REFUND stroops refunded."
