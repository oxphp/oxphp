#!/usr/bin/env bash
#
# Integration test for what a worker is left in when a destructor run by the
# cycle collector from the serve loop throws or ends in a fatal error (worker
# mode, PHP_WORKERS=1).
#
# After a request that ended inside an internal function — usort() here, ended by
# the execution time limit — the serve loop runs the cycle collector before it
# reads the heap. The frame beneath the destructors it runs is oxphp_worker()
# itself, and the worker then leaves the loop, retired or not, and runs the code
# after the oxphp_worker() call. These scenarios leave a cycle behind whose
# destructor misbehaves, and read what that code finds:
#
#   chain     The destructor throws an exception whose own destructor throws
#             again. Nothing catches either, so both stand in the engine until
#             the loop returns, and oxphp_worker() returns with one and skips the
#             code after it.
#   fatalchain
#             The destructor throws an exception whose own destructor ends in a
#             fatal inside a function an observer wraps, while the loop is
#             dropping the exception rather than collecting. Nothing is waiting
#             for that bailout either, and the end handler is pending as in
#             observed.
#   deep      The destructor recurses until the memory limit ends it. The VM
#             stack has grown past its first page, and the cursors it leaves are
#             on different ones, so a call the code after the loop makes writes
#             past the end of the first page.
#   observed  The destructor ends in a fatal inside a function an observer wraps.
#             The observer's end handler is still pending, on a chain that names
#             frames the rewind has since given back, and the request shutdown that
#             follows walks it.
#
# Each must log the end of the code after the loop, the server must stay up, and
# the replacement worker must serve. What retires the worker is asserted too,
# because the leak behind it is sized so that a fatal is the only thing that can:
# a worker retired for the heap's growth would reach the code after the loop on
# its own, and would say nothing about the fatal. The exception is chain, which
# throws and cannot retire the worker, so the leak is large there instead.
#
# NOT wired into run_all.sh or CI (like tests/graceful_drain.sh) — the code after
# the loop runs when a worker exits, which a PHP test cannot see. Run it manually
# after touching the serve loop's collection or the bailout recovery.
#
# Usage: tests/worker_gc_destructor.sh [IMAGE_REF]   (default: oxphp-oxphp:latest)
set -u

IMAGE="${1:-oxphp-oxphp:latest}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FIX="$ROOT/tests/fixtures/gc_destructor"
# Container names carry the PID so two runs on one Docker host do not remove
# each other's containers.
NAME_PREFIX="gc_destructor_$$"
# Ephemeral free port unless the caller pins one via PORT=.
PORT="${PORT:-$(python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()')}"
PASS=0
FAIL=0
TMP="$(mktemp -d)"

ok()   { printf '  \033[32mPASS\033[0m %s\n' "$1"; PASS=$((PASS + 1)); }
bad()  { printf '  \033[31mFAIL\033[0m %s\n' "$1"; FAIL=$((FAIL + 1)); }

cleanup() {
	docker rm -f "${NAME_PREFIX}_chain" "${NAME_PREFIX}_fatalchain" "${NAME_PREFIX}_deep" \
		"${NAME_PREFIX}_observed" >/dev/null 2>&1
	rm -rf "$TMP"
}
trap cleanup EXIT

start_container() {
	# start_container <name>
	docker run -d --name "$1" \
		-e WORKER_FILE=/var/www/html/worker.php \
		-e DOCUMENT_ROOT=/var/www/html \
		-e PHP_WORKERS=1 \
		-e ASYNC_WORKERS=2 \
		-e QUEUE_WAIT_TIMEOUT_MS=20000 \
		-e LOG_LEVEL=info \
		-p ${PORT}:80 \
		-v "$FIX:/var/www/html:ro" \
		"$IMAGE" >/dev/null || return 1
	for _ in $(seq 1 30); do
		curl -fsS "http://localhost:${PORT}/" >/dev/null 2>&1 && return 0
		sleep 1
	done
	return 1
}

wait_for_log() {
	# wait_for_log <name> <fixed string> <max seconds>
	for _ in $(seq 1 "$3"); do
		docker logs "$1" 2>&1 | grep -qF "$2" && return 0
		sleep 1
	done
	return 1
}

scenario() {
	# scenario <kind> <what the destructor logs first> <rows /leak sorts> <what the
	# worker logs as it retires> <whether an observed function is left pending>
	local kind="$1" destructed="$2" rows="$3" retiring="$4" watched="$5" name="${NAME_PREFIX}_$1" logs

	echo "-- $kind"
	if start_container "$name"; then
		ok "$kind: container up"
	else
		bad "$kind: container failed to start"; docker logs "$name" 2>&1 | tail -5; return
	fi

	# A request that has come and gone, so that what follows is judged: the first
	# stretch a worker is busy for never is.
	curl -fsS "http://localhost:${PORT}/" >/dev/null 2>&1

	# /leak parks for two seconds and is then ended inside usort(). /garbage runs
	# and ends in that window, so the collector holds its cycle as a possible
	# root when the request ended inside usort() is the one it runs after.
	curl -s --max-time 30 "http://localhost:${PORT}/leak?rows=${rows}" > "$TMP/leak_$kind" 2>&1 &
	sleep 0.7
	curl -fsS --max-time 10 "http://localhost:${PORT}/garbage?kind=${kind}" > "$TMP/garbage_$kind" 2>&1
	wait

	# The premise: the collector ran the destructor from the serve loop, and not
	# at the end of some request.
	wait_for_log "$name" "$destructed" 15 \
		&& ok "$kind: the collector ran the destructor" \
		|| bad "$kind: the destructor never ran"

	wait_for_log "$name" "$retiring" 15 \
		&& ok "$kind: the worker retired: $retiring" \
		|| bad "$kind: the worker did not log: $retiring"

	wait_for_log "$name" "after-loop-done" 15 \
		&& ok "$kind: the code after oxphp_worker() ran to its end" \
		|| bad "$kind: the code after oxphp_worker() did not run to its end"

	# Give a crash, if there is one, the moment it takes to show.
	sleep 1
	logs="$(docker logs "$name" 2>&1)"
	printf '%s' "$logs" | grep -qE '\[CRASH\]|heap corrupted' \
		&& bad "$kind: the process crashed" \
		|| ok "$kind: the process did not crash"

	[ "$(docker inspect -f '{{.State.Running}}' "$name" 2>/dev/null)" = "true" ] \
		&& ok "$kind: the server is still up" \
		|| bad "$kind: the container is gone (exit $(docker inspect -f '{{.State.ExitCode}}' "$name" 2>/dev/null))"

	if [ "$watched" = "watched" ]; then
		printf '%s' "$logs" | grep -qF "watched-after-ran" \
			&& ok "$kind: the pending end handler was closed" \
			|| bad "$kind: the end handler was never closed"
	fi

	# The replacement worker serves. The pool boots it a moment after the
	# retired one has gone.
	local answer=""
	for _ in $(seq 1 20); do
		answer="$(curl -s --max-time 5 "http://localhost:${PORT}/" 2>/dev/null)"
		[ "$answer" = "ok" ] && break
		sleep 1
	done
	[ "$answer" = "ok" ] \
		&& ok "$kind: a replacement worker answers" \
		|| bad "$kind: nothing answers after the worker left"

	docker rm -f "$name" >/dev/null 2>&1
}

echo "== worker gc destructor ($IMAGE) =="

FATAL_RETIRE="retiring: a destructor ended in a fatal error"
scenario chain "chain-destructed" 40000 "retiring: the heap grew" plain
scenario fatalchain "fatal-chain-destructed" 200 "$FATAL_RETIRE" watched
scenario deep "deep-destructed" 200 "$FATAL_RETIRE" plain
scenario observed "observed-destructed" 200 "$FATAL_RETIRE" watched

echo
echo "== result: $PASS passed, $FAIL failed =="
[ "$FAIL" -eq 0 ]
