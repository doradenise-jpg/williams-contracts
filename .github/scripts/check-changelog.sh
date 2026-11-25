#!/bin/bash
set -euo pipefail

BASE_REF="${CHANGELOG_BASE:-${BASE_REF:-origin/${GITHUB_BASE_REF:-main}}}"
HEAD_REF="${HEAD_REF:-HEAD}"
LIB_RS=contracts/will/src/lib.rs

if ! git rev-parse --verify --quiet "$BASE_REF^{commit}" >/dev/null; then
    echo "::error::Cannot resolve base commit $BASE_REF; checkout must fetch full history (fetch-depth: 0)"
    exit 1
fi

if ! git rev-parse --verify --quiet "$HEAD_REF" >/dev/null; then
    echo "::error::Cannot resolve head ref $HEAD_REF"
    exit 1
fi

LIB_PATH="$LIB_RS"

# Extract the CONTRACT_VERSION assignment value from a file at a given ref.
# Only the assignment line is considered; prose/comment mentions are ignored.
extract_contract_version() {
    local ref="$1"
    git show "$ref:$LIB_PATH" 2>/dev/null \
        | grep -E '^[[:space:]]*(pub[[:space:]]+)?const[[:space:]]+CONTRACT_VERSION[[:space:]]*:' \
        | head -n1 \
        | sed -E 's/.*=[[:space:]]*//' \
        | tr -d '";[:space:]' \
        || true
}

# Extract the crate version from Cargo.toml at a given ref.
extract_crate_version() {
    local ref="$1"
    git show "$ref:contracts/will/Cargo.toml" 2>/dev/null \
        | grep -E '^[[:space:]]*version[[:space:]]*=' \
        | head -n1 \
        | sed -E 's/^[[:space:]]*version[[:space:]]*=[[:space:]]*//' \
        | tr -d '";[:space:]' \
        || true
}

BASE_CONTRACT_VERSION="$(extract_contract_version "$BASE_REF")"
HEAD_CONTRACT_VERSION="$(extract_contract_version "$HEAD_REF")"

CONTRACT_VERSION_CHANGED=0
if [ "$BASE_CONTRACT_VERSION" != "$HEAD_CONTRACT_VERSION" ]; then
    CONTRACT_VERSION_CHANGED=1
fi

if [ "$CONTRACT_VERSION_CHANGED" -eq 1 ]; then
    if ! git diff --name-only "$BASE_REF" "$HEAD_REF" | grep -q 'CHANGELOG.md'; then
        echo "::error::CHANGELOG.md must be updated when CONTRACT_VERSION is changed in $LIB_PATH"
        exit 1
    fi
fi

# Issue #439: CONTRACT_VERSION is compiled into the contract and is immutable
# after deployment. If the constant is bumped in source, the git version tag
# must be bumped in lockstep so SDKs can rely on the reported version to detect
# behavioral changes. Fail the merge when the constant changed but no matching
# version assignment was added/updated in the same change set.
if [ "$CONTRACT_VERSION_CHANGED" -eq 1 ]; then
    BASE_CRATE_VERSION="$(extract_crate_version "$BASE_REF")"
    HEAD_CRATE_VERSION="$(extract_crate_version "$HEAD_REF")"

    if [ -z "$HEAD_CONTRACT_VERSION" ]; then
        echo "::error::Could not determine CONTRACT_VERSION at $HEAD_REF in $LIB_PATH"
        exit 1
    fi

    if [ "$BASE_CRATE_VERSION" = "$HEAD_CRATE_VERSION" ] \
        || [ "$HEAD_CRATE_VERSION" != "$HEAD_CONTRACT_VERSION" ]; then
        echo "::error::CONTRACT_VERSION changed in $LIB_PATH but the crate version was not bumped to match ($HEAD_CONTRACT_VERSION). Bump the crate version (e.g. version = \"$HEAD_CONTRACT_VERSION\") alongside CONTRACT_VERSION."
        exit 1
    fi
fi
