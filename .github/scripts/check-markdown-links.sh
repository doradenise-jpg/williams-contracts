#!/usr/bin/env bash
# check-markdown-links.sh
#
# Offline relative-file and heading-anchor checker for maintained Markdown.
#
# Scans maintained Markdown documents for relative file links and heading
# fragments, resolving them against the containing document. Fails with a
# non-zero exit code and a clear report when a local file or anchor is missing.
#
# Usage:
#   .github/scripts/check-markdown-links.sh            # check maintained docs
#   .github/scripts/check-markdown-links.sh --self-test # run fixture self-tests
#
# Exit codes:
#   0  all links valid (or self-tests passed)
#   1  broken links/anchors found (or self-tests failed)
#   2  usage / environment error

set -euo pipefail

# ---------------------------------------------------------------------------
# Maintained Markdown documents to check.
# ---------------------------------------------------------------------------
MAINTAINED_DOCS=(
  "README.md"
  "CONTRIBUTING.md"
  "CHANGELOG.md"
  "SECURITY.md"
  "spec/README.md"
  "drips wave backlog.md"
)

# ---------------------------------------------------------------------------
# Exemptions: narrow, documented list for HISTORICAL EXTERNAL references only.
#
# Each entry is a literal substring of a link target that is intentionally
# allowed to be broken. These must only cover historical external references
# (e.g. links to artifacts that no longer exist upstream) and must NOT be used
# to silence broken local files or anchors. Keep this list as small as possible
# and add a comment explaining each entry.
# ---------------------------------------------------------------------------
EXEMPTIONS=(
  # Historical external reference to a removed upstream workflow artifact.
  # (Add entries here only with a documented justification.)
)

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

# URL-decode a percent-encoded string (e.g. %20 -> space).
url_decode() {
  local input="$1"
  local out=""
  local i=0
  local len=${#input}
  while (( i < len )); do
    local ch="${input:i:1}"
    if [[ "$ch" == "%" ]] && (( i + 2 < len )); then
      local hex="${input:i+1:2}"
      if [[ "$hex" =~ ^[0-9A-Fa-f]{2}$ ]]; then
        local decoded
        decoded=$(printf "\\x$hex")
        out+="$decoded"
        i=$(( i + 3 ))
        continue
      fi
    fi
    out+="$ch"
    i=$(( i + 1 ))
  done
  printf '%s' "$out"
}

# GitHub-style slugify for a single heading line.
# Lowercase, strip punctuation (keep alnum, spaces, hyphens), spaces -> hyphens.
slugify() {
  local text="$1"
  # Trim leading/trailing whitespace.
  text="${text#"${text%%[![:space:]]*}"}"
  text="${text%"${text##*[![:space:]]}"}"
  # Lowercase.
  text="${text,,}"
  # Remove characters that are not alphanumeric, space, hyphen, or underscore.
  text=$(printf '%s' "$text" | sed -E 's/[^a-z0-9 _-]//g')
  # Spaces -> hyphens.
  text="${text// /-}"
  printf '%s' "$text"
}

# Build the set of valid anchors for a Markdown file, one per line.
# Accounts for duplicate headings by appending -1, -2, etc.
# Also recognizes explicit HTML anchors: <a name="..."> and id="...".
collect_anchors() {
  local file="$1"
  local -A seen=()
  local line heading slug count

  # Heading anchors.
  while IFS= read -r line; do
    # ATX headings: 1-6 leading #, then a space.
    if [[ "$line" =~ ^(#{1,6})[[:space:]]+(.*)$ ]]; then
      heading="${BASH_REMATCH[2]}"
      # Strip trailing closing hashes.
      heading=$(printf '%s' "$heading" | sed -E 's/[[:space:]]+#+[[:space:]]*$//')
      slug=$(slugify "$heading")
      if [[ -n "${seen[$slug]:-}" ]]; then
        count="${seen[$slug]}"
        seen[$slug]=$(( count + 1 ))
        printf '%s-%s\n' "$slug" "$count"
      else
        seen[$slug]=1
        printf '%s\n' "$slug"
      fi
    fi
  done < "$file"

  # Explicit HTML anchors: <a name="..."> and id="...".
  grep -oE '<a[[:space:]]+name="[^"]+"' "$file" 2>/dev/null \
    | sed -E 's/.*name="([^"]+)"/\1/' || true
  grep -oE 'id="[^"]+"' "$file" 2>/dev/null \
    | sed -E 's/id="([^"]+)"/\1/' || true
}

# Check whether a target is exempt.
is_exempt() {
  local target="$1"
  local entry
  for entry in "${EXEMPTIONS[@]}"; do
    if [[ -n "$entry" && "$target" == *"$entry"* ]]; then
      return 0
    fi
  done
  return 1
}

# Extract link targets from a Markdown file.
# Emits one target per line: inline links [text](target) and reference-style
# definitions [id]: target. Skips images and bare autolinks.
extract_links() {
  local file="$1"
  # Inline links: [text](target) — capture the target inside parentheses.
  # Use grep -oE to find all occurrences.
  grep -oE '\]\([^)]+\)' "$file" 2>/dev/null \
    | sed -E 's/^\]\(//; s/\)$//' || true
  # Reference-style definitions: [id]: target
  grep -oE '^[[:space:]]*\[[^]]+\]:[[:space:]]+[^[:space:]]+' "$file" 2>/dev/null \
    | sed -E 's/^[[:space:]]*\[[^]]+\]:[[:space:]]+//' || true
}

# Check a single Markdown file. Returns 0 if all links valid, 1 otherwise.
check_file() {
  local file="$1"
  local dir
  dir=$(dirname "$file")
  local failed=0
  local target path fragment decoded_path resolved

  while IFS= read -r target; do
    [[ -z "$target" ]] && continue

    # Skip external URLs and mailto.
    case "$target" in
      http://*|https://*|mailto:*|tel:*|ftp://*)
        continue
        ;;
    esac

    # Skip pure in-page anchors handled below.
    # Split target into path and fragment.
    path="$target"
    fragment=""
    if [[ "$target" == *"#"* ]]; then
      path="${target%%#*}"
      fragment="${target#*#}"
    fi

    # Decode percent-encoding in the path.
    decoded_path=$(url_decode "$path")

    # Resolve relative path against the containing document's directory.
    if [[ -z "$decoded_path" ]]; then
      resolved="$file"
    elif [[ "$decoded_path" == /* ]]; then
      resolved="$decoded_path"
    else
      resolved="$dir/$decoded_path"
    fi

    # Check file existence (only if a path was given).
    if [[ -n "$decoded_path" ]]; then
      if [[ ! -e "$resolved" ]]; then
        if is_exempt "$target"; then
          continue
        fi
        printf 'BROKEN FILE: %s -> %s (resolved: %s)\n' "$file" "$target" "$resolved" >&2
        failed=1
        continue
      fi
    fi

    # Check fragment anchor if present.
    if [[ -n "$fragment" ]]; then
      # Only check anchors in Markdown files.
      if [[ "$resolved" == *.md ]]; then
        local decoded_fragment
        decoded_fragment=$(url_decode "$fragment")
        if ! collect_anchors "$resolved" | grep -qxF "$decoded_fragment"; then
          if is_exempt "$target"; then
            continue
          fi
          printf 'BROKEN ANCHOR: %s -> %s (missing #%s in %s)\n' \
            "$file" "$target" "$decoded_fragment" "$resolved" >&2
          failed=1
        fi
      fi
    fi
  done < <(extract_links "$file")

  return "$failed"
}

# ---------------------------------------------------------------------------
# Main check mode
# ---------------------------------------------------------------------------
run_check() {
  local overall=0
  local doc

  for doc in "${MAINTAINED_DOCS[@]}"; do
    if [[ ! -f "$doc" ]]; then
      printf 'MISSING DOCUMENT: %s\n' "$doc" >&2
      overall=1
      continue
    fi
    if ! check_file "$doc"; then
      overall=1
    fi
  done

  # Also check docs/**/*.md if present.
  if [[ -d "docs" ]]; then
    while IFS= read -r doc; do
      [[ -z "$doc" ]] && continue
      if ! check_file "$doc"; then
        overall=1
      fi
    done < <(find docs -type f -name '*.md' | sort)
  fi

  if (( overall == 0 )); then
    printf 'All Markdown links and anchors valid.\n'
  else
    printf 'Markdown link/anchor check FAILED.\n' >&2
  fi
  return "$overall"
}

# ---------------------------------------------------------------------------
# Self-test mode using temporary fixtures.
# ---------------------------------------------------------------------------
run_self_test() {
  local tmp
  tmp=$(mktemp -d)
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp'" EXIT

  local failures=0

  # Fixture 1: missing file should fail.
  cat > "$tmp/missing-file.md" <<'EOF'
# Title

[broken](./does-not-exist.md)
EOF
  if check_file "$tmp/missing-file.md" >/dev/null 2>&1; then
    printf 'SELF-TEST FAIL: missing file was not detected\n' >&2
    failures=$(( failures + 1 ))
  fi

  # Fixture 2: duplicate headings produce -1 suffix anchors.
  cat > "$tmp/duplicate.md" <<'EOF'
# Title

## Section

## Section

[first](#section)
[second](#section-1)
EOF
  if ! check_file "$tmp/duplicate.md" >/dev/null 2>&1; then
    printf 'SELF-TEST FAIL: duplicate heading anchors not resolved\n' >&2
    failures=$(( failures + 1 ))
  fi

  # Fixture 3: missing anchor should fail.
  cat > "$tmp/missing-anchor.md" <<'EOF'
# Title

[bad](#nope)
EOF
  if check_file "$tmp/missing-anchor.md" >/dev/null 2>&1; then
    printf 'SELF-TEST FAIL: missing anchor was not detected\n' >&2
    failures=$(( failures + 1 ))
  fi

  # Fixture 4: space-containing and percent-encoded links.
  cat > "$tmp/target file.md" <<'EOF'
# Target
EOF
  cat > "$tmp/spaces.md" <<'EOF'
# Spaces

[raw](./target file.md)
[encoded](./target%20file.md)
EOF
  if ! check_file "$tmp/spaces.md" >/dev/null 2>&1; then
    printf 'SELF-TEST FAIL: space/encoded links not resolved\n' >&2
    failures=$(( failures + 1 ))
  fi

  # Fixture 5: explicit HTML anchors.
  cat > "$tmp/explicit.md" <<'EOF'
# Title

<a name="custom-anchor"></a>
<div id="other-anchor"></div>

[one](#custom-anchor)
[two](#other-anchor)
EOF
  if ! check_file "$tmp/explicit.md" >/dev/null 2>&1; then
    printf 'SELF-TEST FAIL: explicit HTML anchors not recognized\n' >&2
    failures=$(( failures + 1 ))
  fi

  # Fixture 6: URL fragment into another file.
  cat > "$tmp/other.md" <<'EOF'
# Other Heading
EOF
  cat > "$tmp/cross.md" <<'EOF'
# Cross

[link](./other.md#other-heading)
EOF
  if ! check_file "$tmp/cross.md" >/dev/null 2>&1; then
    printf 'SELF-TEST FAIL: cross-file fragment not resolved\n' >&2
    failures=$(( failures + 1 ))
  fi

  if (( failures == 0 )); then
    printf 'Self-tests passed.\n'
    return 0
  fi
  printf 'Self-tests FAILED (%d).\n' "$failures" >&2
  return 1
}

# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------
main() {
  case "${1:-}" in
    --self-test)
      run_self_test
      ;;
    "")
      run_check
      ;;
    *)
      printf 'Usage: %s [--self-test]\n' "$0" >&2
      exit 2
      ;;
  esac
}

main "$@"