#!/usr/bin/env bash
# Verify that tearing down a worker with a request still parked unwinds that
# request cleanly, in the two ways a worker goes away:
#
#   1. the worker exits on its own while a request is parked mid-sleep — the
#      scheduler must unwind it before freeing the state the request runs on;
#   2. the whole process is stopped while a request is parked — it must exit on
#      its own, without a crash and without hanging until the runtime kills it.
#
# Verified from outside the container on purpose: the behaviour under test is
# what happens to a request that is still parked when its worker is torn down,
# and a request cannot assert on its own teardown.
#
# Usage: verify_teardown_unwind.sh [--jsonl]
#   --jsonl  emit one result object per check on stdout instead of a human
#            report, for run_all.sh to fold into its report
set -euo pipefail

JSONL=""
for arg in "$@"; do
	case "$arg" in
		--jsonl) JSONL=1 ;;
		*) echo "Unknown argument: $arg" >&2; exit 1 ;;
	esac
done

cd "$(dirname "$0")/.."
COMPOSE="docker compose -f compose.yml -f compose.fibers.yml"

# One JSONL object per check, matching what run_profile.sh emits so the
# standalone results land in the same report as the PHP suites. In --jsonl mode
# stdout carries only these objects, so everything else goes to stderr.
emit() {
	python3 -c 'import json,sys; print(json.dumps({"test": sys.argv[1], "group": "fibers", "pass": sys.argv[2] == "1", "assertions": [], "error": sys.argv[3], "meta": {}, "profile": "fibers"}, ensure_ascii=False))' "$1" "$2" "$3"
}
ok() {
	if [ -n "$JSONL" ]; then emit "$1" 1 ""; else printf '  \033[32mPASS\033[0m %s\n' "$1"; fi
}
bad() {
	if [ -n "$JSONL" ]; then emit "$1" 0 "$2"; else printf '  \033[31mFAIL\033[0m %s: %s\n' "$1" "$2"; fi
	fail=1
}

$COMPOSE up -d --wait >&2
port="$($COMPOSE port oxphp-fibers 80 | head -1 | cut -d: -f2)"
base="http://127.0.0.1:${port}"

fail=0

# ── 1. Worker teardown with a request parked mid-request ──────────────────
# The request schedules its own worker's exit and then parks, so the worker
# reaches its exit with this fiber suspended mid-request. It must be ended
# rather than dropped, it must not hang, and the worker the pool respawns must
# serve normally afterwards.
$COMPOSE exec -T oxphp-fibers rm -f /tmp/oxphp-parked-shutdown-ran >&2 || true
parked="$(curl -s -w '\n%{http_code}' --max-time 20 "${base}/tests/fibers/fixture_long_park.php?exit=1")"
sleep 2

# A request the worker never ends is answered by the server on its behalf, with
# the generic worker-error page and none of what the request itself produced.
# Its own output coming back instead says the worker gave the request the state
# it had parked with and ended it into its own response.
if printf '%s' "$parked" | grep -q 'parked'; then
	ok "the request parked at worker exit answers with its own output"
else
	bad "the request parked at worker exit answers with its own output" \
		"answered: $(printf '%s' "$parked" | tr '\n' '|')"
fi

# And ending a request runs its shutdown functions — out of the registry it
# registered into, which travels with a parked request rather than staying on
# the worker. The marker is a file because output from a cancelled request is
# refused; see the fixture.
if $COMPOSE exec -T oxphp-fibers test -f /tmp/oxphp-parked-shutdown-ran 2>/dev/null; then
	ok "its shutdown functions run at worker exit"
else
	bad "its shutdown functions run at worker exit" "marker file was never written"
fi

state="$($COMPOSE ps -a --format '{{.State}}' oxphp-fibers)"
if [ "$state" = "running" ]; then
	ok "worker exit under a parked request leaves the server up"
else
	bad "worker exit under a parked request leaves the server up" "container is $state"
fi

after="$(curl -s -o /dev/null -w '%{http_code}' --max-time 20 "${base}/tests/fibers/test_fiber_identity.php")"
if [ "$after" = "200" ]; then
	ok "the respawned worker serves normally"
else
	bad "the respawned worker serves normally" "answered $after (want 200)"
fi

# ── 2. Process teardown with a request parked mid-request ─────────────────
curl -s --max-time 30 "${base}/tests/fibers/fixture_long_park.php" > /dev/null &
curl_pid=$!
sleep 1

start=$(date +%s)
$COMPOSE stop -t 25 oxphp-fibers >&2
elapsed=$(( $(date +%s) - start ))

exit_code="$($COMPOSE ps -a --format '{{.ExitCode}}' oxphp-fibers)"
wait "$curl_pid" || true

log="$($COMPOSE logs oxphp-fibers 2>&1)"
$COMPOSE down -v > /dev/null 2>&1

if [ "$exit_code" = "0" ]; then
	ok "the process stops on its own with a request parked"
else
	bad "the process stops on its own with a request parked" "exit code $exit_code (want 0)"
fi

if [ "$elapsed" -lt 25 ]; then
	ok "the parked request does not hold the shutdown to the stop timeout"
else
	bad "the parked request does not hold the shutdown to the stop timeout" "took ${elapsed}s, was killed at the timeout"
fi

if echo "$log" | grep -qiE "segmentation fault|SIGSEGV|SIGABRT|panicked at"; then
	bad "teardown leaves no crash in the log" "$(echo "$log" | grep -iE "segmentation fault|SIGSEGV|SIGABRT|panicked at" | head -1)"
else
	ok "teardown leaves no crash in the log"
fi

if echo "$log" | grep -qiE "leak|still referenced"; then
	bad "teardown leaves no leak report in the log" "$(echo "$log" | grep -iE "leak|still referenced" | head -1)"
else
	ok "teardown leaves no leak report in the log"
fi

[ -n "$JSONL" ] || [ "$fail" != "0" ] || echo "PASS: teardown unwound a parked request, worker exit and process stop, in ${elapsed}s"
exit "$fail"
