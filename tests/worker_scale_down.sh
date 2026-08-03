#!/usr/bin/env bash
#
# Integration test for dynamic-pool scale-down in worker mode
# (WORKER_FILE + PHP_WORKERS=MIN:MAX).
#
# Retiring a worker means two things, and a build can satisfy one without the
# other, so both are asserted:
#
#   1. The retired thread actually stops. Worker-mode threads used to ignore
#      their per-worker shutdown flag and leave only when the request channel
#      closed — so a "retired" worker kept reading the shared channel for the
#      rest of the process's life while its slot id was already handed to a
#      replacement.
#   2. The process can still exit. The channel closes in SapiExecutor::drop,
#      which runs after drop(runtime), and drop(runtime) waits for the blocking
#      pool — where the retirement join sits. A single retired worker was
#      enough to make SIGTERM never finish.
#
# NOT wired into run_all.sh or CI (like tests/graceful_drain.sh and
# tests/cli_run.sh) — run manually after touching the worker wait path or the
# scale manager.
#
# Usage: tests/worker_scale_down.sh [IMAGE_REF]   (default: oxphp-oxphp:latest)
set -u

IMAGE="${1:-oxphp-oxphp:latest}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FIX="$ROOT/tests/fixtures/scale_down"
PORT="${PORT:-$(python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()')}"
PASS=0
FAIL=0

ok()  { printf '  \033[32mPASS\033[0m %s\n' "$1"; PASS=$((PASS + 1)); }
bad() { printf '  \033[31mFAIL\033[0m %s\n' "$1"; FAIL=$((FAIL + 1)); }

cleanup() { docker rm -f scale_a >/dev/null 2>&1; }
trap cleanup EXIT

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

echo "== worker-mode scale-down ($IMAGE) =="

# PHP_WORKERS=1:2 with a 1s idle threshold. What makes the sequence happen is
# worth stating, because it is calibrated to how the pool behaves today and a
# change there turns this scenario into a no-op rather than a failure:
#
#   scale-up   — the manager grows the pool when no worker looks idle, and a
#                worker-mode worker that has served a request currently never
#                looks idle again, so the single readiness request below is
#                enough and no further traffic is needed;
#   scale-down — it retires a worker idle past the threshold, which for the
#                same reason can only be the one that never received a request.
#
# Both halves are asserted, so if that behaviour changes the run stops at the
# assertion that no longer holds instead of quietly proving nothing.
docker run -d --name scale_a \
	-e WORKER_FILE=/var/www/html/worker_scale.php \
	-e DOCUMENT_ROOT=/var/www/html \
	-e PHP_WORKERS=1:2 \
	-e PHP_WORKERS_IDLE_SECONDS=1 \
	-e LOG_LEVEL=info \
	-p ${PORT}:80 \
	-v "$FIX:/var/www/html:ro" \
	"$IMAGE" >/dev/null || { bad "container failed to start"; exit 1; }

up=0
for _ in $(seq 1 30); do
	curl -fsS "http://localhost:${PORT}/" >/dev/null 2>&1 && { up=1; break; }
	sleep 1
done
[ "$up" = 1 ] && ok "container up" || { bad "container never answered"; docker logs scale_a 2>&1 | tail -20; exit 1; }

# The readiness request above is the only one this scenario sends, and that is
# deliberate: a worker-mode worker that has served becomes non-retirable, so
# handing one to the worker the manager is about to spawn would remove the very
# thing the scenario needs. Wait for the growth without driving it, and assert
# it — a scale-up that silently did not happen must not be reported later as a
# scale-down that did not happen.
grew=0
for _ in $(seq 1 30); do
	if docker logs scale_a 2>&1 | grep -q "Scale-up: spawned worker"; then grew=1; break; fi
	sleep 0.5
done
[ "$grew" = 1 ] \
	&& ok "pool scaled up to its ceiling" \
	|| { bad "pool never scaled up — recalibrate the scenario before reading anything below"; docker logs scale_a 2>&1 | tail -20; exit 1; }

# Now wait the scale manager out: its 5s scale-down cooldown, plus the 1s idle
# threshold the extra worker has to cross, plus room for a slow host.
sleep 15

LOGS="$(docker logs scale_a 2>&1)"

# Anchored on the closing quote of the JSON message field: the unanchored form
# is also a prefix of the "…thread stopped" line asserted below, which would let
# the completion stand in for the event it is supposed to be evidence about.
printf '%s' "$LOGS" | grep -q '"message":"Scale-down: retired worker"' \
	&& ok "scale-down fired" \
	|| { bad "scale-down never fired — the scenario proves nothing"; printf '%s\n' "$LOGS" | tail -20; exit 1; }

# The mechanism, not the symptom. "Worker mode thread stopped" is the last line
# a worker-mode thread prints before returning, and it predates this scenario —
# so its absence here means the thread is still running, not that the build
# lacks the line. Before SIGTERM, the only thing that can produce it is a
# retirement the thread honoured.
printf '%s' "$LOGS" | grep -q "Worker mode thread stopped" \
	&& ok "retired worker thread actually stopped" \
	|| bad "retired worker thread never stopped — it is still on the request channel"

# Same fact from the pool's side: the line is printed after the retirement join
# returns, so a wedged join is silent. Ops signal as much as test signal.
printf '%s' "$LOGS" | grep -q "Scale-down: retired worker thread stopped" \
	&& ok "retirement join returned" \
	|| bad "retirement join never returned"

# Retiring one worker must not cost the pool its ability to answer.
BODY="$(curl -fsS --max-time 10 "http://localhost:${PORT}/" 2>&1)"
printf '%s' "$BODY" | grep -q "^worker " \
	&& ok "pool still serves after scale-down" \
	|| bad "request after scale-down failed (got: $(printf '%s' "$BODY" | head -c 60))"

# The symptom. A retired worker parked on a channel that only closes after
# drop(runtime) keeps the runtime's blocking pool alive forever.
docker kill -s TERM scale_a >/dev/null
ELAPSED=$(wait_exit_seconds scale_a 30)
docker ps --format '{{.Names}}' | grep -q '^scale_a$' \
	&& bad "still running ${ELAPSED}s after SIGTERM — shutdown is wedged" \
	|| ok "exited ${ELAPSED}s after SIGTERM"

echo
echo "== $PASS passed, $FAIL failed =="
[ "$FAIL" -eq 0 ]
