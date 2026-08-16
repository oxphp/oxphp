#!/usr/bin/env bash
# tests/scripts/test-assertions.sh
#
# Fixture-based unit test for tests/lib/assertions.sh.
#
# `run_php_test` decides what a failing test tells its reader. When the response
# body is not the JSON a test emits, that decision is all the reader gets: the
# request is gone, and the report is the only place the body was ever visible.
# A server-side fatal, a rendered error page and a request that never answered
# all reach this branch, and telling them apart is the whole point of reading a
# failure.
#
# The stub replaces `http_request`, so nothing here needs a container: the
# runner library executes on the host, and every case below is a fixed body
# handed to it directly.
#
# Usage: tests/scripts/test-assertions.sh

set -euo pipefail

LIB_DIR="$(cd "$(dirname "$0")/../lib" && pwd)"
# shellcheck source=/dev/null
source "${LIB_DIR}/common.sh"
# shellcheck source=/dev/null
source "${LIB_DIR}/assertions.sh"

# ── Stub ─────────────────────────────────────────────────────
# Answers with whatever STUB_STATUS/STUB_BODY hold, in the shape common.sh's
# http_request produces. Arguments are ignored — the URL never gets fetched.
export STUB_STATUS=200
export STUB_BODY=""

http_request() {
    python3 -c '
import json, os
print(json.dumps({"status": int(os.environ["STUB_STATUS"]),
                  "headers": {},
                  "body": os.environ["STUB_BODY"]}, ensure_ascii=False))
'
}

# run_case <status> <body> — returns the JSONL line run_php_test emits.
run_case() {
    STUB_STATUS="$1"
    STUB_BODY="$2"
    run_php_test "http://stub" "shared/test_stub"
}

# field <jsonl> <key> — one field of the emitted line, as text.
field() {
    printf '%s' "$1" | python3 -c '
import sys, json
print(json.load(sys.stdin).get(sys.argv[1]))
' "$2"
}

expect() {
    local name="$1" actual="$2" expected="$3"
    if [[ "$actual" != "$expected" ]]; then
        echo "FAIL: $name"
        echo "  expected: $expected"
        echo "  actual:   $actual"
        exit 1
    fi
    echo "PASS: $name"
}

expect_contains() {
    local name="$1" actual="$2" needle="$3"
    if [[ "$actual" != *"$needle"* ]]; then
        echo "FAIL: $name"
        echo "  expected to contain: $needle"
        echo "  actual:              $actual"
        exit 1
    fi
    echo "PASS: $name"
}

# --- Case 1: a body that is not JSON keeps its content ---
# The reason a test failed lives in the body and nowhere else. Dropping it
# leaves a status code, and the body has to be re-fetched by hand — by which
# time the run that produced it is over.
FATAL='Fatal error: Maximum execution time of 0 seconds exceeded in /var/www/html/public/tests/shared/test_stub.php on line 12'
RESULT=$(run_case 500 "$FATAL")
expect         "case 1 does not pass"      "$(field "$RESULT" pass)"  "False"
expect_contains "case 1 names the status"  "$(field "$RESULT" error)" "HTTP 500"
expect_contains "case 1 keeps the body"    "$(field "$RESULT" error)" "$FATAL"

# --- Case 2: an empty body says so ---
# A request that never answered (curl failure, status 0) and one that answered
# with an unparseable body are different failures. Reported identically, the
# first reads as a broken test rather than a server that stopped responding.
RESULT=$(run_case 0 "")
expect         "case 2 does not pass"       "$(field "$RESULT" pass)"  "False"
expect_contains "case 2 names the status"   "$(field "$RESULT" error)" "HTTP 0"
expect_contains "case 2 reports empty body" "$(field "$RESULT" error)" "empty"

# --- Case 3: a long body is cut, and the cut is announced ---
# Error pages run to kilobytes of HTML. The excerpt has to end somewhere, but a
# silent cut turns a truncated body into a body that looks complete.
LONG=$(python3 -c 'print("HEAD-MARKER " + "x" * 4000 + " TAIL-MARKER", end="")')
RESULT=$(run_case 500 "$LONG")
ERR=$(field "$RESULT" error)
expect_contains "case 3 keeps the head" "$ERR" "HEAD-MARKER"
if [[ "$ERR" == *"TAIL-MARKER"* ]]; then
    echo "FAIL: case 3 truncates a long body"
    echo "  4000-char body was reported in full"
    exit 1
fi
echo "PASS: case 3 truncates a long body"
expect_contains "case 3 announces the cut" "$ERR" "chars"

# --- Case 4: an escape sequence in the body stays text ---
# The report prints this field straight to a terminal. A body carrying ANSI
# codes — an error page, a coloured stack trace — would otherwise repaint the
# run summary around it, and the colour the reporter chose for the line is the
# only thing saying the line is a failure.
RESULT=$(run_case 500 "$(printf 'Fatal \033[31mRED\033[0m error\001')")
ERR=$(field "$RESULT" error)
if [[ "$ERR" == *$'\033'* || "$ERR" == *$'\001'* ]]; then
    echo "FAIL: case 4 spells control bytes out"
    echo "  raw control bytes survived into the report: $(printf '%q' "$ERR")"
    exit 1
fi
echo "PASS: case 4 spells control bytes out"
expect_contains "case 4 keeps the surrounding text" "$ERR" "Fatal "
expect_contains "case 4 names the escaped byte"     "$ERR" "u001b"

# --- Case 5: the line stays one line of JSON whatever the body contains ---
# Everything downstream — the profile injection in run_profile.sh, all three
# reports — reads this output back with json.loads, one object per line. A body
# carrying the characters JSON encodes (quote, backslash, newline, tab) is the
# case where hand-built output would split the line or corrupt the field.
# shellcheck disable=SC1003  # the trailing backslash is the point, not an escape
NASTY='he said "x" \ then
	a tab and a trailing backslash \'
RESULT=$(run_case 500 "$NASTY")
expect "case 5 emits exactly one line" "$(printf '%s' "$RESULT" | wc -l | tr -d ' ')" "0"
expect "case 5 round-trips the body" \
    "$(field "$RESULT" error)" "HTTP 500: non-JSON response: $NASTY"

# --- Case 6: a test's own JSON still passes through untouched ---
# The branches above must not move for a response the runner can already read.
TEST_JSON='{"test":"test_stub","group":"shared","pass":true,"assertions":[],"error":null,"meta":{"probe":42}}'
RESULT=$(run_case 200 "$TEST_JSON")
expect "case 6 passes the body through verbatim" "$RESULT" "$TEST_JSON"

# --- Case 7: the smoke form (tests/php/shared/*) is unchanged ---
RESULT=$(run_case 200 'debug line
OK
')
expect "case 7 OK passes" "$(field "$RESULT" pass)" "True"

RESULT=$(run_case 200 'OK
FAIL: stuck — got=1998 sent=1998 of 2000 after 12s
')
expect         "case 7 FAIL does not pass"  "$(field "$RESULT" pass)"  "False"
expect_contains "case 7 FAIL keeps reason"  "$(field "$RESULT" error)" "got=1998 sent=1998"

echo "ALL TESTS PASSED"
