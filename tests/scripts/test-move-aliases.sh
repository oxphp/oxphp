#!/usr/bin/env bash
# tests/scripts/test-move-aliases.sh
#
# Fixture-based unit test for scripts/move-aliases.sh.
# Generates a temporary supported-versions.yml and asserts the dry-run
# output matches expected alias mappings for each (oxphp, PHP, distro)
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
  - "3.22"
default_php: "8.4"
EOF

REGISTRY="ghcr.io/oxphp/oxphp"

# A docker stub that records every call and fails: no case may reach the
# registry, and a script that tries to is caught rather than obeyed.
mkdir -p "$TMPDIR/bin"
cat > "$TMPDIR/bin/docker" <<STUB
#!/usr/bin/env bash
echo "docker \$*" >> "$TMPDIR/docker-calls"
exit 1
STUB
chmod +x "$TMPDIR/bin/docker"
export PATH="$TMPDIR/bin:$PATH"

# Run the script under test and capture sorted dry-run output
run_case() {
    local oxphp="$1"
    local php="$2"
    local distro="$3"
    CONFIG_FILE="$FIXTURE" REGISTRY="$REGISTRY" \
        "$SCRIPT" "$oxphp" "$php" "$distro" --dry-run 2>&1 \
        | { grep '^DRY RUN:' || true; } | LC_ALL=C sort
}

# Same, in --list mode: the whole output, unfiltered, so that anything but
# alias names would show up as a difference.
run_list() {
    local out rc=0
    out=$(CONFIG_FILE="$FIXTURE" REGISTRY="$REGISTRY" \
        "$SCRIPT" "$1" "$2" "$3" --list) || rc=$?
    if [[ -n "$out" ]]; then printf '%s\n' "$out" | LC_ALL=C sort; fi
    if [[ "$rc" -ne 0 ]]; then echo "exit $rc"; fi
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
        FAILED=1
        return
    fi
    echo "PASS: $name"
}
FAILED=0

# --- Case 1: latest oxphp + default PHP (8.4) + latest alpine ---
# Should move: version-scoped (0.3.0-php8.4, 0.3-php8.4),
# latest-for-php (php8.4), default-oriented (0.3.0, latest).
CASE1_ACTUAL=$(run_case 0.3.0 8.4 alpine3.23)
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
CASE2_ACTUAL=$(run_case 0.3.0 8.5 alpine3.23)
CASE2_EXPECTED=$(printf '%s\n' \
    "DRY RUN: ghcr.io/oxphp/oxphp:0.3-php8.5 -> ghcr.io/oxphp/oxphp:0.3.0-php8.5-alpine3.23" \
    "DRY RUN: ghcr.io/oxphp/oxphp:0.3.0-php8.5 -> ghcr.io/oxphp/oxphp:0.3.0-php8.5-alpine3.23" \
    "DRY RUN: ghcr.io/oxphp/oxphp:php8.5 -> ghcr.io/oxphp/oxphp:0.3.0-php8.5-alpine3.23" \
    | LC_ALL=C sort)
expect "case 2 (latest oxphp + non-default PHP)" "$CASE2_ACTUAL" "$CASE2_EXPECTED"

# --- Case 3: previous oxphp + latest alpine ---
# Should move: version-scoped only (0.2.0-php8.4, 0.2-php8.4).
# NO latest-oriented aliases (0.2.0 is not latest).
CASE3_ACTUAL=$(run_case 0.2.0 8.4 alpine3.23)
CASE3_EXPECTED=$(printf '%s\n' \
    "DRY RUN: ghcr.io/oxphp/oxphp:0.2-php8.4 -> ghcr.io/oxphp/oxphp:0.2.0-php8.4-alpine3.23" \
    "DRY RUN: ghcr.io/oxphp/oxphp:0.2.0-php8.4 -> ghcr.io/oxphp/oxphp:0.2.0-php8.4-alpine3.23" \
    | LC_ALL=C sort)
expect "case 3 (previous oxphp)" "$CASE3_ACTUAL" "$CASE3_EXPECTED"

# --- Case 4: latest oxphp + default PHP + Debian trixie ---
# Every alias carries the codename and `trixie` stands in for `latest`.
# 0.3.0-php8.4-trixie is the canonical tag itself, so it is not re-created.
CASE4_ACTUAL=$(run_case 0.3.0 8.4 trixie)
CASE4_EXPECTED=$(printf '%s\n' \
    "DRY RUN: ghcr.io/oxphp/oxphp:0.3-php8.4-trixie -> ghcr.io/oxphp/oxphp:0.3.0-php8.4-trixie" \
    "DRY RUN: ghcr.io/oxphp/oxphp:0.3.0-trixie -> ghcr.io/oxphp/oxphp:0.3.0-php8.4-trixie" \
    "DRY RUN: ghcr.io/oxphp/oxphp:php8.4-trixie -> ghcr.io/oxphp/oxphp:0.3.0-php8.4-trixie" \
    "DRY RUN: ghcr.io/oxphp/oxphp:trixie -> ghcr.io/oxphp/oxphp:0.3.0-php8.4-trixie" \
    | LC_ALL=C sort)
expect "case 4 (latest oxphp + default PHP, trixie)" "$CASE4_ACTUAL" "$CASE4_EXPECTED"

# --- Case 5: latest oxphp + non-default PHP + trixie ---
CASE5_ACTUAL=$(run_case 0.3.0 8.5 trixie)
CASE5_EXPECTED=$(printf '%s\n' \
    "DRY RUN: ghcr.io/oxphp/oxphp:0.3-php8.5-trixie -> ghcr.io/oxphp/oxphp:0.3.0-php8.5-trixie" \
    "DRY RUN: ghcr.io/oxphp/oxphp:php8.5-trixie -> ghcr.io/oxphp/oxphp:0.3.0-php8.5-trixie" \
    | LC_ALL=C sort)
expect "case 5 (latest oxphp + non-default PHP, trixie)" "$CASE5_ACTUAL" "$CASE5_EXPECTED"

# --- Case 6: previous oxphp + trixie ---
CASE6_ACTUAL=$(run_case 0.2.0 8.4 trixie)
CASE6_EXPECTED="DRY RUN: ghcr.io/oxphp/oxphp:0.2-php8.4-trixie -> ghcr.io/oxphp/oxphp:0.2.0-php8.4-trixie"
expect "case 6 (previous oxphp, trixie)" "$CASE6_ACTUAL" "$CASE6_EXPECTED"

# --- Case 7: an Alpine version that is not the latest listed ---
# Alpine aliases name no Alpine version, so only the latest one moves them.
CASE7_ACTUAL=$(run_case 0.3.0 8.4 alpine3.22)
expect "case 7 (older alpine moves nothing)" "$CASE7_ACTUAL" ""

# --- Case 8: --list prints the alias names the move would create, only ---
names_of() { sed 's/^DRY RUN: \([^ ]*\) -> .*/\1/' | LC_ALL=C sort; }
CASE8A_ACTUAL=$(run_list 0.3.0 8.4 alpine3.23)
expect "case 8a (--list, alpine)" "$CASE8A_ACTUAL" "$(printf '%s\n' "$CASE1_EXPECTED" | names_of)"
CASE8B_ACTUAL=$(run_list 0.3.0 8.4 trixie)
expect "case 8b (--list, trixie)" "$CASE8B_ACTUAL" "$(printf '%s\n' "$CASE4_EXPECTED" | names_of)"
CASE8C_ACTUAL=$(run_list 0.3.0 8.4 alpine3.22)
expect "case 8c (--list, older alpine)" "$CASE8C_ACTUAL" ""

# --- Case 9: an unknown flag is refused, not taken as a real move ---
CASE9_RC=0
CONFIG_FILE="$FIXTURE" REGISTRY="$REGISTRY" \
    "$SCRIPT" 0.3.0 8.4 alpine3.23 --lsit >/dev/null 2>&1 || CASE9_RC=$?
expect "case 9 (unknown flag exits 2)" "$CASE9_RC" "2"

expect "no case called docker" "$(cat "$TMPDIR/docker-calls" 2>/dev/null || true)" ""

if [[ "$FAILED" -ne 0 ]]; then
    echo "SOME TESTS FAILED"
    exit 1
fi
echo "ALL TESTS PASSED"
