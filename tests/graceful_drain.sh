#!/usr/bin/env bash
#
# Integration test for graceful drain on SIGTERM (worker mode, PHP_WORKERS=1
# so every request multiplexes onto one PHP worker).
#
# Scenario A (soft drain): long-lived streams in every suspend shape — SLEEP
# multiplexed twice, AWAIT, empty-flush, try/catch — are cancelled promptly
# and uncatchably, INCLUDING the first stream (fast-path fiber), while an
# ordinary in-flight request is left alone to finish with its full response,
# a request that finished its response via oxphp_finish_request() keeps its
# background work, and the sweep kills must not trip the worker's
# consecutive-error breaker. The container must exit well before
# DRAIN_TIMEOUT_SECONDS.
#
# Scenario B (hard cancel): a CPU-bound request that never suspends or
# flushes survives the soft phase and is killed by the broadcast vm_interrupt
# kick once the drain deadline passes; a cooperatively suspended request is
# killed by the deadline sweep.
#
# Scenario C (flush-path kills): streams that flush in a tight loop and never
# suspend are unreachable to the sweep — only the stream-flush path's
# self-cancel ends them. Three in a row must not trip the worker's
# consecutive-error breaker, or the worker error-exits mid-drain and takes the
# ordinary in-flight request down with it.
#
# NOT wired into run_all.sh or CI (like tests/cli_run.sh) — run manually after
# touching the drain machinery (fiber sweep, cancel plumbing, drain latches).
#
# Usage: tests/graceful_drain.sh [IMAGE_REF]   (default: oxphp-oxphp:latest)
set -u

IMAGE="${1:-oxphp-oxphp:latest}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FIX="$ROOT/tests/fixtures/drain"
# Ephemeral free port unless the caller pins one via PORT=.
PORT="${PORT:-$(python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()')}"
PASS=0
FAIL=0
TMP="$(mktemp -d)"

ok()   { printf '  \033[32mPASS\033[0m %s\n' "$1"; PASS=$((PASS + 1)); }
bad()  { printf '  \033[31mFAIL\033[0m %s\n' "$1"; FAIL=$((FAIL + 1)); }

cleanup() {
	docker rm -f drain_a drain_b drain_c >/dev/null 2>&1
	rm -rf "$TMP"
}
trap cleanup EXIT

start_container() {
	# start_container <name> <drain_timeout_seconds>
	docker run -d --name "$1" \
		-e WORKER_FILE=/var/www/html/worker_drain.php \
		-e DOCUMENT_ROOT=/var/www/html \
		-e PHP_WORKERS=1 \
		-e ASYNC_WORKERS=2 \
		-e DRAIN_TIMEOUT_SECONDS="$2" \
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

wait_exit_seconds() {
	# wait_exit_seconds <name> <max> — echoes seconds until the container exits.
	local start
	start=$(date +%s)
	for _ in $(seq 1 "$2"); do
		docker ps --format '{{.Names}}' | grep -q "^$1\$" || break
		sleep 1
	done
	echo $(( $(date +%s) - start ))
}

echo "== graceful drain ($IMAGE) =="

# ── Scenario A: soft drain ───────────────────────────────────
if start_container drain_a 30; then
	ok "A: container up"
else
	bad "A: container failed to start"; docker logs drain_a 2>&1 | tail -5; exit 1
fi

# Warm-up request: completes and parks its fiber on the free list, so the
# first stream below reuses it (regression: stale request_cancel_ptr).
curl -fsS "http://localhost:${PORT}/" >/dev/null 2>&1

# First stream arrives on an idle worker → fast path (regression: fiber
# created there never captured its cancel cell and survived the drain).
curl -N -s --max-time 40 "http://localhost:${PORT}/sse"   > "$TMP/a1" 2>&1 &
sleep 1
# The rest arrive while fibers are active → event-loop path.
curl -N -s --max-time 40 "http://localhost:${PORT}/sse"   > "$TMP/a2" 2>&1 &
curl -N -s --max-time 40 "http://localhost:${PORT}/await" > "$TMP/a3" 2>&1 &
curl -N -s --max-time 40 "http://localhost:${PORT}/empty" > "$TMP/a4" 2>&1 &
curl -N -s --max-time 40 "http://localhost:${PORT}/catch" > "$TMP/a5" 2>&1 &
sleep 2

# A stream that will FINISH its response (oxphp_finish_request) mid-drain and
# then do background work: the closing flush must not self-cancel it, and once
# finished the suspended fiber counts as ordinary — the soft sweep must spare
# it. Its native pre-finish sleep spans the SIGTERM below.
curl -N -s --max-time 30 "http://localhost:${PORT}/bg?ms=2500" > "$TMP/bg" 2>&1 &
sleep 0.5

# Ordinary requests in flight at SIGTERM: must complete, not be cancelled —
# one blocked in a native sleep, one suspended cooperatively (no streaming;
# the soft sweep must spare it even though it is a suspended fiber).
curl -fsS --max-time 20 "http://localhost:${PORT}/short?ms=4000" > "$TMP/short" 2>&1 &
SHORT_PID=$!
curl -fsS --max-time 20 "http://localhost:${PORT}/pause?s=2"     > "$TMP/pause" 2>&1 &
PAUSE_PID=$!
sleep 1

docker kill -s TERM drain_a >/dev/null
ELAPSED=$(wait_exit_seconds drain_a 25)

# Streams must be gone quickly — nowhere near the 30s drain timeout.
[ "$ELAPSED" -le 12 ] \
	&& ok "A: exited ${ELAPSED}s after SIGTERM (<=12s)" \
	|| bad "A: exit took ${ELAPSED}s"

wait "$SHORT_PID"
grep -q "short-done" "$TMP/short" \
	&& ok "A: in-flight short request finished with its response" \
	|| bad "A: short request lost (got: $(head -c 60 "$TMP/short"))"

wait "$PAUSE_PID"
grep -q "pause-done" "$TMP/pause" \
	&& ok "A: cooperatively suspended request survived the soft sweep" \
	|| bad "A: suspended non-streaming request was killed at t=0 (got: $(head -c 60 "$TMP/pause"))"

LOGS_A="$(docker logs drain_a 2>&1)"
CANCELLED=$(printf '%s' "$LOGS_A" | grep -c "Request cancelled (shutdown)")
[ "$CANCELLED" -ge 5 ] \
	&& ok "A: all 5 streams cancelled uncatchably ($CANCELLED)" \
	|| bad "A: only $CANCELLED/5 'Request cancelled (shutdown)' lines"

printf '%s' "$LOGS_A" | grep -q "bg-done" \
	&& ok "A: post-finish_request background work survived the drain" \
	|| bad "A: finished request's background work was cancelled ('bg-done' missing)"

printf '%s' "$LOGS_A" | grep -q "All connections drained" \
	&& ok "A: drained cleanly (no timeout)" \
	|| bad "A: 'All connections drained' missing"

printf '%s' "$LOGS_A" | grep -q "Drain timeout reached" \
	&& bad "A: drain hit its deadline — soft drain failed" \
	|| ok "A: deadline never reached"

# 5 sweep kills in a row must NOT trip the worker's consecutive-error breaker
# (exit_reason=3) — that would error-exit the worker mid-drain and destroy the
# still-live ordinary requests. Kills are administrative, not handler errors.
printf '%s' "$LOGS_A" | grep -Eq '"exit_reason":3|exit_reason=3|Dead workers detected' \
	&& bad "A: drain kills tripped the consecutive-error breaker (worker error-exit mid-drain)" \
	|| ok "A: no worker error-exit during the drain window"

docker rm -f drain_a >/dev/null 2>&1

# ── Scenario B: hard cancel at the deadline ──────────────────
if start_container drain_b 3; then
	ok "B: container up"
else
	bad "B: container failed to start"; exit 1
fi

# CPU-bound fiber (blocks the scheduler; only the vm_interrupt kick reaches
# it) plus a long cooperative sleep with no streaming (survives the soft
# phase; only the deadline sweep may end it).
curl -N -s --max-time 30 "http://localhost:${PORT}/spin"      > "$TMP/spin"  2>&1 &
sleep 0.5
curl -N -s --max-time 30 "http://localhost:${PORT}/pause?s=60" > "$TMP/hang" 2>&1 &
sleep 1

docker kill -s TERM drain_b >/dev/null
ELAPSED=$(wait_exit_seconds drain_b 20)

# Deadline is 3s + 2s unwind beat; well under the CPU loop's natural end (never).
[ "$ELAPSED" -le 10 ] \
	&& ok "B: exited ${ELAPSED}s after SIGTERM (deadline kick worked)" \
	|| bad "B: exit took ${ELAPSED}s — CPU-bound request not cancelled"

LOGS_B="$(docker logs drain_b 2>&1)"
printf '%s' "$LOGS_B" | grep -q "Drain timeout reached, cancelling in-flight requests" \
	&& ok "B: hard-cancel phase entered" \
	|| bad "B: hard-cancel log line missing"

CANCELLED_B=$(printf '%s' "$LOGS_B" | grep -c "Request cancelled (shutdown)")
[ "$CANCELLED_B" -ge 2 ] \
	&& ok "B: CPU-bound + suspended stragglers cancelled at the deadline ($CANCELLED_B)" \
	|| bad "B: only $CANCELLED_B/2 deadline cancellations recorded"

printf '%s' "$LOGS_B" | grep -Eq '"exit_reason":3|exit_reason=3|Dead workers detected' \
	&& bad "B: deadline kills tripped the consecutive-error breaker" \
	|| ok "B: no worker error-exit during the drain window"

docker rm -f drain_b >/dev/null 2>&1

# ── Scenario C: flush-path kills don't trip the breaker ──────
if start_container drain_c 30; then
	ok "C: container up"
else
	bad "C: container failed to start"; exit 1
fi

# An ordinary suspended request keeps a fiber alive, so the worker stays on the
# event-loop path — the only path that syncs the scheduler's consecutive-error
# counter and can act on it. It must still be alive at the end.
curl -fsS --max-time 30 "http://localhost:${PORT}/pause?s=6" > "$TMP/c_pause" 2>&1 &
C_PAUSE_PID=$!
sleep 0.5

# Three tight-flush streams. The worker runs them one at a time (none ever
# suspends); the two behind the first wait in the request queue and are
# dispatched as each predecessor is killed — three flush-path kills in a row.
for i in 1 2 3; do
	curl -N -s --max-time 30 "http://localhost:${PORT}/tight" > "$TMP/c_tight$i" 2>&1 &
done
sleep 1

docker kill -s TERM drain_c >/dev/null
ELAPSED=$(wait_exit_seconds drain_c 25)

[ "$ELAPSED" -le 12 ] \
	&& ok "C: exited ${ELAPSED}s after SIGTERM (<=12s)" \
	|| bad "C: exit took ${ELAPSED}s"

wait "$C_PAUSE_PID"
grep -q "pause-done" "$TMP/c_pause" \
	&& ok "C: ordinary request survived three flush-path kills" \
	|| bad "C: ordinary request lost — worker died mid-drain (got: $(head -c 60 "$TMP/c_pause"))"

LOGS_C="$(docker logs drain_c 2>&1)"
CANCELLED_C=$(printf '%s' "$LOGS_C" | grep -c "Request cancelled (shutdown)")
[ "$CANCELLED_C" -ge 3 ] \
	&& ok "C: all 3 tight-flush streams cancelled ($CANCELLED_C)" \
	|| bad "C: only $CANCELLED_C/3 tight-flush streams cancelled"

printf '%s' "$LOGS_C" | grep -Eq '"exit_reason":3|exit_reason=3|Dead workers detected' \
	&& bad "C: flush-path kills tripped the consecutive-error breaker" \
	|| ok "C: no worker error-exit during the drain window"

printf '%s' "$LOGS_C" | grep -q "Drain timeout reached" \
	&& bad "C: drain hit its deadline — flush-path cancel failed" \
	|| ok "C: deadline never reached"

echo
echo "== result: $PASS passed, $FAIL failed =="
[ "$FAIL" -eq 0 ]
