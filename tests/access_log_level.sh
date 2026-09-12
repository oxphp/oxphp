#!/usr/bin/env bash
#
# Integration test for the access log against the general log level.
#
# The access log is written through the tracing subscriber, whose filter used to
# be whatever LOG_LEVEL named — one level applying to every target at once.
# That made ACCESS_LOG a setting the log level could veto: a server asked for an
# access log and told to keep quiet otherwise
# wrote nothing, reported `all` on /config while doing it, and said nothing
# about the contradiction. Every check here therefore counts the lines the
# server actually wrote, because the setting's own echo is exactly what cannot
# tell the two states apart.
#
#   A: ACCESS_LOG=all under a quiet LOG_LEVEL writes one entry per request.
#   B: ACCESS_LOG=error under the same level still logs only the errors — the
#      directive frees the target from the level, it does not override the mode.
#   C: the general log stays quiet throughout A and B: the server's own INFO
#      chatter is still filtered, so the entries in A came from the access log
#      being freed rather than from the level having been turned up wholesale.
#      C0 runs the same count against a server at LOG_LEVEL=info first, so that
#      the zeroes C asserts are known to be a filtered log rather than a count
#      that cannot move.
#   D: an operator who names the target in RUST_LOG keeps the last word — the
#      entries stay away, and the server says so at startup instead of leaving
#      an enabled access log silent.
#   E: with ACCESS_LOG unset the filter is left alone and there is nothing to
#      report. E and A sit on either side of the startup warning's two
#      conditions: A has an access log that works, E has no access log asked
#      for, so each scenario leaves exactly one of the two as the only thing
#      keeping the warning quiet.
#   F: the report is itself a log line, so a filter quiet enough to drop the
#      access log can drop the report with it. At a level below WARN it has to
#      arrive by another route or the contradiction is silent again — which is
#      the whole defect, one level up.
#
# Static files are used deliberately: they reach the same RequestComplete event
# the access log hangs off, without a PHP worker that could fail for reasons
# this test is not about.
#
# NOT wired into run_all.sh or CI (like tests/graceful_drain.sh and
# tests/cli_run.sh) — run manually after touching logging or the access log.
#
# Usage: tests/access_log_level.sh [IMAGE_REF]   (default: oxphp-oxphp:latest)
set -u

IMAGE="${1:-oxphp-oxphp:latest}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FIX="$ROOT/tests/fixtures/access_log"
# Ephemeral free port unless the caller pins one via PORT=.
PORT="${PORT:-$(python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()')}"
# Per-invocation, so two runs of this script cannot tear down each other's
# container mid-scenario.
SRV="access_log_srv_$$"
PASS=0
FAIL=0

ok()  { printf '  \033[32mPASS\033[0m %s\n' "$1"; PASS=$((PASS + 1)); }
bad() { printf '  \033[31mFAIL\033[0m %s\n' "$1"; FAIL=$((FAIL + 1)); }

cleanup() { docker rm -f "$SRV" >/dev/null 2>&1; }
trap cleanup EXIT

# start_container <ACCESS_LOG> <LOG_LEVEL> <RUST_LOG> — an empty argument
# leaves that variable unset, which is not the same as setting it to "".
start_container() {
	local access="$1" level="$2" rustlog="$3"
	docker rm -f "$SRV" >/dev/null 2>&1
	set -- -d --name "$SRV" -e DOCUMENT_ROOT=/var/www/html -e PHP_WORKERS=1
	if [ -n "$access" ]; then set -- "$@" -e "ACCESS_LOG=$access"; fi
	if [ -n "$level" ]; then set -- "$@" -e "LOG_LEVEL=$level"; fi
	if [ -n "$rustlog" ]; then set -- "$@" -e "RUST_LOG=$rustlog"; fi
	docker run "$@" -p "${PORT}":80 -v "$FIX:/var/www/html:ro" "$IMAGE" >/dev/null || return 1
	for _ in $(seq 1 30); do
		curl -fsS "http://localhost:${PORT}/ping.txt" >/dev/null 2>&1 && return 0
		sleep 1
	done
	return 1
}

# Lines matching a pattern in everything the container has written so far. The
# readiness probe above is itself a logged request, so every count is taken as a
# delta across the requests a scenario makes, never as an absolute.
logged() { docker logs "$SRV" 2>&1 | grep -c "$1"; }
entries() { logged 'request completed'; }

# INFO lines that are not access log entries. The access log emits at INFO
# itself, so counting the level alone cannot tell "the one target was freed"
# from "everything got louder" — and on a build where the entries never arrive
# it reads as quiet for the wrong reason. The entries are excluded by their
# message because the formatter is configured `with_target(false)`, so the
# target does not reach the JSON — which makes that message text load-bearing
# here, and this check has to follow it if it is ever reworded. Under
# LOG_LEVEL=info this server writes a couple of dozen such lines before the
# first request even arrives, so the check has something to catch.
server_info() { docker logs "$SRV" 2>&1 | grep '"level":"INFO"' | grep -vc 'request completed'; }

# get <path> <count> — <count> sequential requests, responses discarded.
get() {
	local path="$1" count="$2" i
	for i in $(seq 1 "$count"); do
		curl -s -o /dev/null "http://localhost:${PORT}${path}"
	done
	# The subscriber writes from a background thread and docker buffers on top
	# of that; without this the count races the writer rather than the server.
	sleep 1
}

echo "== access log vs log level ($IMAGE) =="

# ── C (control): the non-access INFO count can reach something ───────
# The two C checks below assert this count is zero. A zero it reaches because
# the pattern matches nothing at all — a changed log format, `with_target`
# turned back on, a container that came up broken — satisfies them just as
# well, and neither would notice. So the same helper runs first against a
# server whose general log is at INFO, where those lines are what it is
# supposed to find.
if start_container "all" "info" ""; then
	ok "C0: container up (ACCESS_LOG=all, LOG_LEVEL=info)"
else
	bad "C0: container failed to start"
	docker logs "$SRV" 2>&1 | tail -5 >&2
	exit 1
fi

C0="$(server_info)"
if [ "$C0" -ge 1 ]; then
	ok "C0: $C0 non-access INFO lines under LOG_LEVEL=info — the C checks can see them"
else
	bad "C0: none under LOG_LEVEL=info — the C checks below cannot fail and prove nothing"
fi

# ── A: an enabled access log outlives a quiet log level ──────────────
if start_container "all" "warn" ""; then
	ok "A: container up (ACCESS_LOG=all, LOG_LEVEL=warn)"
else
	bad "A: container failed to start"
	docker logs "$SRV" 2>&1 | tail -5 >&2
	exit 1
fi

BEFORE="$(entries)"
get /ping.txt 5
A_DELTA=$(($(entries) - BEFORE))
if [ "$A_DELTA" -eq 5 ]; then
	ok "A: 5 requests → 5 access log entries"
else
	bad "A: 5 requests → $A_DELTA access log entries (LOG_LEVEL=warn is still vetoing ACCESS_LOG)"
fi

# ── C (first half): the rest of the log is still quiet ───────────────
# INFO on the server's own targets must stay filtered, or A would pass because
# the whole log got louder rather than because the access log was let through.
if [ "$(logged 'no access log entries will be written')" -eq 0 ]; then
	ok "A: nothing reported — the access log is on and reaching the log"
else
	bad "A: the startup warning fired while the access log was writing entries"
fi

C1="$(server_info)"
if [ "$C1" -eq 0 ]; then
	ok "C: no INFO from the server's own targets under LOG_LEVEL=warn"
else
	bad "C: $C1 non-access INFO lines — the directive turned the general log up"
fi

# ── B: the mode still decides which requests are logged ──────────────
if start_container "error" "warn" ""; then
	ok "B: container up (ACCESS_LOG=error, LOG_LEVEL=warn)"
else
	bad "B: container failed to start"
	docker logs "$SRV" 2>&1 | tail -5 >&2
	exit 1
fi

BEFORE="$(entries)"
get /ping.txt 3
get /missing.txt 2
B_DELTA=$(($(entries) - BEFORE))
if [ "$B_DELTA" -eq 2 ]; then
	ok "B: 3× 200 + 2× 404 → 2 entries, the errors only"
else
	bad "B: 3× 200 + 2× 404 → $B_DELTA entries, expected 2"
fi

C2="$(server_info)"
if [ "$C2" -eq 0 ]; then
	ok "C: still no INFO from the server's own targets in error mode"
else
	bad "C: $C2 non-access INFO lines — the directive turned the general log up"
fi

# ── D: an explicit RUST_LOG directive wins, and is reported ──────────
if start_container "all" "" "warn,access_log=off"; then
	ok "D: container up (ACCESS_LOG=all, RUST_LOG=warn,access_log=off)"
else
	bad "D: container failed to start"
	docker logs "$SRV" 2>&1 | tail -5 >&2
	exit 1
fi

BEFORE="$(entries)"
get /ping.txt 3
D_DELTA=$(($(entries) - BEFORE))
if [ "$D_DELTA" -eq 0 ]; then
	ok "D: the operator's own directive still switches the target off"
else
	bad "D: $D_DELTA entries — an explicit RUST_LOG directive was overridden"
fi

if [ "$(logged 'no access log entries will be written')" -ge 1 ]; then
	ok "D: the contradiction is reported at startup"
else
	bad "D: an enabled access log writes nothing and the server says nothing"
fi

if [ "$(logged '"access_log":"all"')" -ge 1 ] && [ "$(logged 'access_log=off')" -ge 1 ]; then
	ok "D: the report names the ACCESS_LOG value and the filter that drops it"
else
	bad "D: the report names neither the setting nor the filter, so it cannot be acted on"
fi

# ── E: nothing asked for, nothing added, nothing reported ────────────
if start_container "" "warn" ""; then
	ok "E: container up (ACCESS_LOG unset, LOG_LEVEL=warn)"
else
	bad "E: container failed to start"
	docker logs "$SRV" 2>&1 | tail -5 >&2
	exit 1
fi

BEFORE="$(entries)"
get /ping.txt 3
E_DELTA=$(($(entries) - BEFORE))
if [ "$E_DELTA" -eq 0 ]; then
	ok "E: an access log nobody asked for stays off"
else
	bad "E: $E_DELTA entries with ACCESS_LOG unset"
fi

if [ "$(logged 'no access log entries will be written')" -eq 0 ]; then
	ok "E: and nothing is reported about a setting nobody set"
else
	bad "E: the startup warning fired with ACCESS_LOG unset"
fi

# ── F: the report survives a filter that would drop it ───────────────
if start_container "all" "" "error,access_log=off"; then
	ok "F: container up (ACCESS_LOG=all, RUST_LOG=error,access_log=off)"
else
	bad "F: container failed to start"
	docker logs "$SRV" 2>&1 | tail -5 >&2
	exit 1
fi

BEFORE="$(entries)"
get /ping.txt 3
F_DELTA=$(($(entries) - BEFORE))
if [ "$F_DELTA" -eq 0 ]; then
	ok "F: the operator's own directive still switches the target off"
else
	bad "F: $F_DELTA entries — an explicit RUST_LOG directive was overridden"
fi

if [ "$(logged 'no access log entries will be written')" -ge 1 ]; then
	ok "F: the contradiction is reported even below WARN"
else
	bad "F: nothing written and nothing said — the report was filtered away too"
fi

if [ "$(logged 'access_log=all')" -ge 1 ] && [ "$(logged 'access_log=off')" -ge 1 ]; then
	ok "F: that report names the ACCESS_LOG value and the filter that drops it"
else
	bad "F: the fallback report names neither the setting nor the filter"
fi

echo
echo "== result: $PASS passed, $FAIL failed =="
[ "$FAIL" -eq 0 ]
