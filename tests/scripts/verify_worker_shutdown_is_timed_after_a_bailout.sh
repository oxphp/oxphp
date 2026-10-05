#!/usr/bin/env bash
# Verify that what a worker script runs after a fatal error has ended its serve
# loop between two requests — its own shutdown functions — is under the time
# limit the requests started with.
#
# While it serves, a worker keeps the thread's execution timer for the requests
# it runs, armed only while one of them is running, so between two requests
# nothing is armed. A fatal error raised there leaves the serve loop by a long
# jump: a destructor run by the cycle collector the worker runs every hundredth
# request is one way. A shutdown function that destructor registered first is
# left on the thread for the worker script's own shutdown, and PHP runs a
# script's shutdown functions before it stops the script's timer, so one that
# blocks is ended by the limit — and would otherwise hold the worker's thread
# for as long as it blocks.
#
# Verified from outside the container on purpose: the code under test runs after
# the worker's serve loop has gone, where no request is left to report on it.
#
# Usage: verify_worker_shutdown_is_timed_after_a_bailout.sh [--jsonl]
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
# Same compose project as run_all.sh, so a check run by hand drives the
# profile of this checkout rather than one shared with other checkouts.
# shellcheck source=../lib/common.sh
source lib/common.sh
COMPOSE="docker compose -f compose.yml -f compose.workertimer.yml"

emit() {
	python3 -c 'import json,sys; print(json.dumps({"test": sys.argv[1], "group": "workertimer", "pass": sys.argv[2] == "1", "assertions": [], "error": sys.argv[3], "meta": {}, "profile": "workertimer"}, ensure_ascii=False))' "$1" "$2" "$3"
}
ok() {
	if [ -n "$JSONL" ]; then emit "$1" 1 ""; else printf '  \033[32mPASS\033[0m %s\n' "$1"; fi
}
bad() {
	if [ -n "$JSONL" ]; then emit "$1" 0 "$2"; else printf '  \033[31mFAIL\033[0m %s: %s\n' "$1" "$2"; fi
	fail=1
}

# A worker that has served nothing yet, so that the requests below are the ones
# it counts towards the hundredth.
$COMPOSE up -d --wait --force-recreate >&2
port="$($COMPOSE port oxphp-workertimer 80 | head -1 | cut -d: -f2)"
base="http://127.0.0.1:${port}"

fail=0

count_of() {
	python3 -c 'import json,sys
try: print(json.loads(sys.stdin.read())["request_count"])
except Exception: print("")'
}

# The cycle first, then requests that leave nothing behind until the worker has
# served a hundred: the collector runs after the hundredth. The last one raises
# its own limit, so a fatal that names it rather than the profile's two seconds
# would say the worker script was left on that request's limit.
left="$(curl -s --max-time 10 "${base}/tests/workertimer/fixture_cycle_with_a_fatal_destructor.php")"
served="$(printf '%s' "$left" | count_of)"
if [ "$served" = "1" ]; then
	ok "premise: the request that leaves the cycle is the fresh worker's first"
else
	bad "premise: the request that leaves the cycle is the fresh worker's first" "answered: $left"
fi

# All of them on the worker that served the first. A worker recycled in between
# starts its count again and takes the cycle with it, so a count that does not go
# up by one ends the wait as a failed premise rather than being waited out.
answered=""
for _ in $(seq 2 99); do
	[ "$served" -lt 99 ] 2>/dev/null || break
	answered="$(curl -s --max-time 10 "${base}/?action=count")"
	next="$(printf '%s' "$answered" | count_of)"
	[ "$next" = "$((served + 1))" ] || break
	served="$next"
done
if [ "$served" = "99" ]; then
	ok "premise: the worker that left the cycle served every request up to the hundredth"
else
	bad "premise: the worker that left the cycle served every request up to the hundredth" \
		"after ${served:-no count} it answered: ${answered:-nothing}"
fi
last="$(curl -s --max-time 10 "${base}/tests/workertimer/fixture_cycle_with_a_fatal_destructor.php?action=raise_limit")"

# The probe spins for 8 s; the limit, when it is armed, ends it after 2.
log=""
for _ in $(seq 1 60); do
	log="$($COMPOSE logs oxphp-workertimer 2>&1)"
	if echo "$log" | grep -qE 'worker-shutdown-probe: finished|Maximum execution time'; then
		break
	fi
	sleep 0.25
done

if echo "$log" | grep -q 'a destructor the collector ran ended in a fatal error' \
	&& echo "$log" | grep -q 'oxphp-worker-shutdown-probe: started'; then
	ok "premise: a fatal error between requests ended the serve loop and the worker script's shutdown ran"
else
	bad "premise: a fatal error between requests ended the serve loop and the worker script's shutdown ran" \
		"hundredth request answered: ${last}; log: $(echo "$log" | grep -E 'oxphp-worker|PHP' | tail -3 | tr '\n' '|')"
fi

# Granted: it was not ended the moment it started.
if echo "$log" | grep -q 'oxphp-worker-shutdown-probe: still running after 1s'; then
	ok "the worker script's shutdown function ran for a second"
else
	bad "the worker script's shutdown function ran for a second" "no such line in the log"
fi

# Bounded: by the limit the requests started with.
if echo "$log" | grep -q 'oxphp-worker-shutdown-probe: finished'; then
	bad "the worker script's shutdown function was ended by the limit the requests started with" \
		"it ran to its end: $(echo "$log" | grep -o 'oxphp-worker-shutdown-probe: finished[^"]*' | head -1)"
elif echo "$log" | grep -q 'Maximum execution time of 2 seconds exceeded'; then
	ok "the worker script's shutdown function was ended by the limit the requests started with"
else
	bad "the worker script's shutdown function was ended by the limit the requests started with" \
		"$(echo "$log" | grep -o 'Maximum execution time[^"]*' | head -1)"
fi

if echo "$log" | grep -qiE "segmentation fault|SIGSEGV|SIGABRT|panicked at"; then
	bad "the worker's exit leaves no crash in the log" "$(echo "$log" | grep -iE "segmentation fault|SIGSEGV|SIGABRT|panicked at" | head -1)"
else
	ok "the worker's exit leaves no crash in the log"
fi

$COMPOSE down -v > /dev/null 2>&1

[ -n "$JSONL" ] || [ "$fail" != "0" ] || echo "PASS: the worker script's shutdown ran under the limit after its serve loop was ended by a fatal error"
exit "$fail"
