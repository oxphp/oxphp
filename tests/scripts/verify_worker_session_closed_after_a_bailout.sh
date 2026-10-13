#!/usr/bin/env bash
# Verify that a session a request holds open while it is parked is written and
# closed — and the store's lock on it let go — when a fatal error between two
# slices ends the worker's serve loop.
#
# A request parked with its session open has that session taken off the thread
# while it waits, so that the requests run beside it find none there. A fatal
# error raised between two slices leaves the serve loop by a long jump: a
# destructor run by the cycle collector the worker runs every hundredth request
# is one way. What follows is the worker script's own shutdown, which writes and
# closes a session it finds open, as the end of any script does — and finds
# none unless the parked request's is put back first. Left off the thread, the
# files handler's descriptor keeps its lock on that session for as long as the
# process lives, and every later request with that cookie waits for it holding
# a worker thread.
#
# The destructor that ends in the fatal error also opens a session of its own
# first, as PHP run outside any request can: what is standing on the thread is
# given back before the parked request's session is put there, or its lock is
# lost the same way.
#
# Verified from outside the container on purpose: the code under test runs after
# the worker's serve loop has gone, where no request is left to report on it.
#
# Usage: verify_worker_session_closed_after_a_bailout.sh [--jsonl]
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
COMPOSE="docker compose -f compose.yml -f compose.fibers.yml"

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

# A worker that has served nothing yet, so that the requests below are the ones
# it counts towards the hundredth.
$COMPOSE up -d --wait --force-recreate >&2
port="$($COMPOSE port oxphp-fibers 80 | head -1 | cut -d: -f2)"
base="http://127.0.0.1:${port}"

fail=0

field() {
	python3 -c 'import json,sys
try:
    v = json.loads(sys.stdin.read())
    for k in sys.argv[1].split("."):
        v = v[k]
    print(json.dumps(v) if not isinstance(v, str) else v)
except Exception:
    print("")' "$1"
}

sid="oxphpbailout$(date +%s)${RANDOM}"
orphan="${sid}orphan"

# The cycle first, then requests that leave nothing behind until the worker has
# served ninety-nine; the hundredth opens the session and parks with it.
left="$(curl -s --max-time 10 "${base}/tests/fibers/fixture_session_bailout_cycle.php?orphan=${orphan}")"
served="$(printf '%s' "$left" | field request_count)"
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
	next="$(printf '%s' "$answered" | field request_count)"
	[ "$next" = "$((served + 1))" ] || break
	served="$next"
done
if [ "$served" = "99" ]; then
	ok "premise: the worker that left the cycle served every request up to the hundredth"
else
	bad "premise: the worker that left the cycle served every request up to the hundredth" \
		"after ${served:-no count} it answered: ${answered:-nothing}"
fi

held="$(curl -s --max-time 20 -b "PHPSESSID=${sid}" "${base}/tests/fibers/fixture_session_held_at_a_bailout.php" || true)"

# Served by the worker the pool boots in place of the one the fatal error ended;
# the queue lets it wait for the boot.
probe="$(curl -s --max-time 20 "${base}/tests/fibers/fixture_session_bailout_probe.php?id=${sid}&orphan=${orphan}")"

if [ "$(printf '%s' "$probe" | field held.request_count)" = "100" ] \
	&& [ "$(printf '%s' "$probe" | field held.id)" = "$sid" ]; then
	ok "premise: the hundredth request opened the session under the client's cookie"
else
	bad "premise: the hundredth request opened the session under the client's cookie" "probe: $probe"
fi

# Both edges of the window: the collector ran once the hundredth request was in,
# and that request never got back from its park — so the fatal error came while
# it was parked holding the session.
if [ "$(printf '%s' "$probe" | field destructor.request_count)" = "100" ]; then
	ok "premise: the collector ran the destructor once the hundredth request was in"
else
	bad "premise: the collector ran the destructor once the hundredth request was in" "probe: $probe"
fi
if printf '%s' "$held" | grep -q 'the end of the park'; then
	bad "premise: the request holding the session was still parked when the fatal error ended the worker" \
		"it answered: $held"
else
	ok "premise: the request holding the session was still parked when the fatal error ended the worker"
fi
if [ "$(printf '%s' "$probe" | field destructor.orphan_status)" = "2" ]; then
	ok "premise: the destructor had a session of its own open when it ended in the fatal error"
else
	bad "premise: the destructor had a session of its own open when it ended in the fatal error" "probe: $probe"
fi
if [ "$(printf '%s' "$probe" | field request_count)" = "1" ]; then
	ok "premise: the probe is served by the worker that replaced the one the fatal error ended"
else
	bad "premise: the probe is served by the worker that replaced the one the fatal error ended" "probe: $probe"
fi

free="$(printf '%s' "$probe" | field session.free)"
if [ "$free" = "true" ]; then
	ok "the store's lock on the session is let go once the fatal error has ended the serve loop"
else
	bad "the store's lock on the session is let go once the fatal error has ended the serve loop" \
		"exists: $(printf '%s' "$probe" | field session.exists), free: ${free:-?}"
fi

if printf '%s' "$probe" | field session.contents | grep -q '^marker|'; then
	ok "the worker script's shutdown wrote the session the parked request held open"
else
	bad "the worker script's shutdown wrote the session the parked request held open" \
		"file holds: $(printf '%s' "$probe" | field session.contents)"
fi

if [ "$(printf '%s' "$probe" | field orphan.free)" = "true" ] \
	&& printf '%s' "$probe" | field orphan.contents | grep -q '^orphan|'; then
	ok "the session the destructor left open is written and its lock let go too"
else
	bad "the session the destructor left open is written and its lock let go too" \
		"exists: $(printf '%s' "$probe" | field orphan.exists), free: $(printf '%s' "$probe" | field orphan.free), file holds: $(printf '%s' "$probe" | field orphan.contents)"
fi

# The symptom, asked only once the lock is known to be free: with it still taken
# the request would wait for it holding the one worker.
if [ "$free" = "true" ]; then
	next="$(curl -s --max-time 10 "${base}/tests/fibers/fixture_session_bailout_probe.php?id=${sid}&start=1")"
	if [ "$(printf '%s' "$next" | field marker)" = "written by the worker script shutdown" ]; then
		ok "the next request with that cookie opens the session and finds what was written"
	else
		bad "the next request with that cookie opens the session and finds what was written" "answered: $next"
	fi
else
	bad "the next request with that cookie opens the session and finds what was written" \
		"not asked: the lock is still taken, and the request would wait for it holding the worker"
fi

log="$($COMPOSE logs oxphp-fibers 2>&1)"
if echo "$log" | grep -qiE "segmentation fault|SIGSEGV|SIGABRT|panicked at"; then
	bad "the worker's exit leaves no crash in the log" "$(echo "$log" | grep -iE "segmentation fault|SIGSEGV|SIGABRT|panicked at" | head -1)"
else
	ok "the worker's exit leaves no crash in the log"
fi

$COMPOSE down -v > /dev/null 2>&1

[ -n "$JSONL" ] || [ "$fail" != "0" ] || echo "PASS: a session held open by a parked request is written and closed when a fatal error ends the serve loop"
exit "$fail"
