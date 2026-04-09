#!/usr/bin/env bash
# tests/scripts/test-move-aliases.sh
#
# Fixture-based unit test for scripts/move-aliases.sh.
# Generates a temporary supported-versions.yml and asserts the dry-run
# output matches expected alias mappings for each (oxphp, PHP, alpine)
# triple.
#
# Usage: tests/scripts/test-move-aliases.sh

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SCRIPT="${REPO_ROOT}/scripts/move-aliases.sh"
TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

# Build a fixture supported-versions.yml (note: includes a hypothetical
# PHP 8.5 entry so we can test the "latest + non-default PHP" case; the
# real repo's supported-versions.yml only lists 8.4 until oxphp's Rust
# FFI bindings are updated for the 8.5 ABI).
FIXTURE="$TMPDIR/supported-versions.yml"
cat > "$FIXTURE" <<'EOF'
oxphp_versions:
  - "0.3.0"
  - "0.2.0"
php_versions:
  - "8.4"
  - "8.5"
alpine_versions:
  - "3.23"
default_php: "8.4"
EOF

REGISTRY="ghcr.io/oxphp/oxphp"

# Run the script under test and capture sorted dry-run output
run_case() {
    local oxphp="$1"
    local php="$2"
    local alpine="$3"
    CONFIG_FILE="$FIXTURE" REGISTRY="$REGISTRY" \
        "$SCRIPT" "$oxphp" "$php" "$alpine" --dry-run 2>&1 \
        | grep '^DRY RUN:' | LC_ALL=C sort
}

expect() {
    local name="$1"
    local actual="$2"
    local expected="$3"
    if [[ "$actual" != "$expected" ]]; then
        echo "FAIL: $name"
        echo "  expected:"
        printf '%s\n' "$expected" | sed 's/^/    /'
        echo "  actual:"
        printf '%s\n' "$actual" | sed 's/^/    /'
        exit 1
    fi
    echo "PASS: $name"
}

# --- Case 1: latest oxphp + default PHP (8.4) + latest alpine ---
# Should move: version-scoped (0.3.0-php8.4, 0.3-php8.4),
# latest-for-php (php8.4), default-oriented (0.3.0, latest).
CASE1_ACTUAL=$(run_case 0.3.0 8.4 3.23)
CASE1_EXPECTED=$(printf '%s\n' \
    "DRY RUN: ghcr.io/oxphp/oxphp:0.3-php8.4 -> ghcr.io/oxphp/oxphp:0.3.0-php8.4-alpine3.23" \
    "DRY RUN: ghcr.io/oxphp/oxphp:0.3.0 -> ghcr.io/oxphp/oxphp:0.3.0-php8.4-alpine3.23" \
    "DRY RUN: ghcr.io/oxphp/oxphp:0.3.0-php8.4 -> ghcr.io/oxphp/oxphp:0.3.0-php8.4-alpine3.23" \
    "DRY RUN: ghcr.io/oxphp/oxphp:latest -> ghcr.io/oxphp/oxphp:0.3.0-php8.4-alpine3.23" \
    "DRY RUN: ghcr.io/oxphp/oxphp:php8.4 -> ghcr.io/oxphp/oxphp:0.3.0-php8.4-alpine3.23" \
    | LC_ALL=C sort)
expect "case 1 (latest oxphp + default PHP)" "$CASE1_ACTUAL" "$CASE1_EXPECTED"

# --- Case 2: latest oxphp + non-default PHP (8.5) + latest alpine ---
# Should move: version-scoped, latest-for-php. NOT default-oriented
# (latest and 0.3.0 belong to PHP 8.4, not 8.5).
CASE2_ACTUAL=$(run_case 0.3.0 8.5 3.23)
CASE2_EXPECTED=$(printf '%s\n' \
    "DRY RUN: ghcr.io/oxphp/oxphp:0.3-php8.5 -> ghcr.io/oxphp/oxphp:0.3.0-php8.5-alpine3.23" \
    "DRY RUN: ghcr.io/oxphp/oxphp:0.3.0-php8.5 -> ghcr.io/oxphp/oxphp:0.3.0-php8.5-alpine3.23" \
    "DRY RUN: ghcr.io/oxphp/oxphp:php8.5 -> ghcr.io/oxphp/oxphp:0.3.0-php8.5-alpine3.23" \
    | LC_ALL=C sort)
expect "case 2 (latest oxphp + non-default PHP)" "$CASE2_ACTUAL" "$CASE2_EXPECTED"

# --- Case 3: previous oxphp + latest alpine ---
# Should move: version-scoped only (0.2.0-php8.4, 0.2-php8.4).
# NO latest-oriented aliases (0.2.0 is not latest).
CASE3_ACTUAL=$(run_case 0.2.0 8.4 3.23)
CASE3_EXPECTED=$(printf '%s\n' \
    "DRY RUN: ghcr.io/oxphp/oxphp:0.2-php8.4 -> ghcr.io/oxphp/oxphp:0.2.0-php8.4-alpine3.23" \
    "DRY RUN: ghcr.io/oxphp/oxphp:0.2.0-php8.4 -> ghcr.io/oxphp/oxphp:0.2.0-php8.4-alpine3.23" \
    | LC_ALL=C sort)
expect "case 3 (previous oxphp)" "$CASE3_ACTUAL" "$CASE3_EXPECTED"

echo "ALL TESTS PASSED"
