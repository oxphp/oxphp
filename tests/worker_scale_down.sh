#!/usr/bin/env bash
#
# Integration test for the dynamic worker pool (PHP_WORKERS=MIN:MAX): worker
# mode for the parts about retiring a worker that has served, traditional mode
# for the part about not retiring one that is still serving.
#
# Four things are asserted, because a build can satisfy any one of them
# without the others:
#
#   1. The pool sizes itself from real demand. A worker that answered a
#      request and then went quiet has to look idle again, or the manager
#      sees a pool with nobody idle in it and keeps spawning.
#   2. The retired thread actually stops. Worker-mode threads used to ignore
#      their per-worker shutdown flag and leave only when the request channel
#      closed — so a "retired" worker kept reading the shared channel for the
#      rest of the process's life while its slot id was already handed to a
#      replacement.
#   3. A worker still serving is not retired out from under its request — the
#      idle stamp records when work arrived, not whether it finished.
#   4. The process can still exit. The channel closes in SapiExecutor::drop,
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
PASS=0
FAIL=0

ok()  { printf '  \033[32mPASS\033[0m %s\n' "$1"; PASS=$((PASS + 1)); }
bad() { printf '  \033[31mFAIL\033[0m %s\n' "$1"; FAIL=$((FAIL + 1)); }

free_port() { python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()'; }

cleanup() { docker rm -f scale_a scale_b scale_c >/dev/null 2>&1; }
trap cleanup EXIT

# start_pool <name> <port> — PHP_WORKERS=1:2 with a 1s idle threshold, so the
# whole grow/shrink cycle fits inside the manager's 500ms tick and 5s
# scale-down cooldown instead of the 30s default.
start_pool() {
	docker run -d --name "$1" \
		-e WORKER_FILE=/var/www/html/worker_scale.php \
		-e DOCUMENT_ROOT=/var/www/html \
		-e PHP_WORKERS=1:2 \
		-e PHP_WORKERS_IDLE_SECONDS=1 \
		-e LOG_LEVEL=info \
		-p "$2":80 \
		-v "$FIX:/var/www/html:ro" \
		"$IMAGE" >/dev/null || return 1
	for _ in $(seq 1 30); do
		curl -fsS "http://localhost:$2/" >/dev/null 2>&1 && return 0
		sleep 1
	done
	return 1
}

# worker_of <port> — one request, echoes the id of the worker that answered
# (the fixture prints "worker <id>").
worker_of() {
	curl -fsS --max-time 10 "http://localhost:$1/" 2>/dev/null | awk '/^worker /{print $2; exit}'
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

echo "== dynamic worker pool ($IMAGE) =="

# ── Part 1: a quiet pool stops growing ───────────────────────────────────────
#
# One request, then silence. A single arrival can still land inside the 200ms
# window the manager treats as "busy" and buy one spawn — that is the sizing
# rule doing its job, and it is not what this asserts. What must happen next is
# that the pool settles: with nobody sending anything, every worker ages past
# the threshold and no further spawn can be justified.
#
# This is the half that fails when the idle stamp is written in a different
# unit than the manager reads. The age computed for a worker that has served is
# then always zero, so it never becomes idle again — the manager sees a pool
# with no idle worker in it, spawns, retires the untouched spare a moment later
# for being the only one that can look idle, and starts over. Left alone the
# cycle repeats for the life of the process, so counting spawns across a quiet
# window separates it from a pool that has settled.
PORT_A="${PORT:-$(free_port)}"
start_pool scale_a "$PORT_A" \
	&& ok "container up (quiet pool)" \
	|| { bad "container never answered"; docker logs scale_a 2>&1 | tail -20; exit 1; }

# Let the one request settle, then count spawns across a window wider than the
# 5s scale-down cooldown the cycle turns on.
sleep 4
SPAWNS_BEFORE=$(docker logs scale_a 2>&1 | grep -c "Scale-up: spawned worker")
sleep 12
SPAWNS_AFTER=$(docker logs scale_a 2>&1 | grep -c "Scale-up: spawned worker")
[ "$SPAWNS_BEFORE" = "$SPAWNS_AFTER" ] \
	&& ok "quiet pool stopped growing (spawns held at $SPAWNS_AFTER)" \
	|| bad "quiet pool kept spawning ($SPAWNS_BEFORE then $SPAWNS_AFTER) — a served worker never looks idle"

docker rm -f scale_a >/dev/null 2>&1

# ── Part 2: a worker that has served gets retired ────────────────────────────
PORT_B="${PORT2:-$(free_port)}"
start_pool scale_b "$PORT_B" \
	&& ok "container up (loaded pool)" \
	|| { bad "container never answered"; docker logs scale_b 2>&1 | tail -20; exit 1; }

# Drive traffic until every worker in the pool has an arrival inside the
# manager's 200ms idle window — that, and only that, is what makes it grow.
# Collect the ids that answered: the retirement below has to name one of them,
# and on a build where a served worker never ages that is exactly what cannot
# happen.
SERVED=""
END=$(( $(date +%s) + 6 ))
while [ "$(date +%s)" -lt "$END" ]; do
	id="$(worker_of "$PORT_B")"
	[ -n "$id" ] || continue
	case " $SERVED " in *" $id "*) ;; *) SERVED="$SERVED $id" ;; esac
done
[ -n "$SERVED" ] \
	&& ok "traffic served by worker(s):$SERVED" \
	|| { bad "no request was answered — the scenario proves nothing"; docker logs scale_b 2>&1 | tail -20; exit 1; }

docker logs scale_b 2>&1 | grep -q "Scale-up: spawned worker" \
	&& ok "pool grew under load" \
	|| { bad "pool never grew under load — recalibrate the scenario before reading anything below"; docker logs scale_b 2>&1 | tail -20; exit 1; }

# Traffic has stopped. Wait the manager out: the 1s idle threshold, its 5s
# scale-down cooldown, and room for a slow host.
sleep 15

LOGS="$(docker logs scale_b 2>&1)"

# Anchored on the closing quote of the JSON message field: the unanchored form
# is also a prefix of the "…thread stopped" line asserted below, which would let
# the completion stand in for the event it is supposed to be evidence about.
RETIRE_LINE="$(printf '%s\n' "$LOGS" | grep '"message":"Scale-down: retired worker"' | head -1)"
[ -n "$RETIRE_LINE" ] \
	&& ok "scale-down fired" \
	|| { bad "scale-down never fired — every worker that served still looks busy"; printf '%s\n' "$LOGS" | tail -20; exit 1; }

# "Worker mode thread stopped" is the last line a worker-mode thread prints
# before returning, and it predates this scenario — so its absence here means
# the thread is still running, not that the build lacks the line. Before
# SIGTERM, the only thing that can produce it is a retirement the thread
# honoured.
printf '%s' "$LOGS" | grep -q "Worker mode thread stopped" \
	&& ok "retired worker thread actually stopped" \
	|| bad "retired worker thread never stopped — it is still on the request channel"

# Same fact from the pool's side: the line is printed after the retirement join
# returns, so a wedged join is silent. Ops signal as much as test signal.
JOIN_LINE="$(printf '%s\n' "$LOGS" | grep '"message":"Scale-down: retired worker thread stopped"' | head -1)"
[ -n "$JOIN_LINE" ] \
	&& ok "retirement join returned" \
	|| bad "retirement join never returned"

# The mechanism, not the symptom. A build whose idle age is stuck at zero for
# anyone who served can still retire a worker — the spare it spawned and never
# fed — and that retirement is indistinguishable from this one by count alone.
# So the retired id, which the join line names, has to be one of the ids that
# answered a request above.
RETIRED_ID="$(printf '%s' "$JOIN_LINE" | sed -n 's/.*"retired_id":\([0-9][0-9]*\).*/\1/p')"
# The emptiness check is not redundant: `$SERVED` is accumulated as " 0 1", so
# the haystack starts with two spaces and an empty id would match the pattern.
if [ -n "$RETIRED_ID" ] && { case " $SERVED " in *" $RETIRED_ID "*) true ;; *) false ;; esac; }; then
	ok "retired worker $RETIRED_ID had served requests"
else
	bad "retired worker '${RETIRED_ID:-<no id in log>}' never served; ids that did:$SERVED"
fi

# Retiring one worker must not cost the pool its ability to answer.
BODY="$(curl -fsS --max-time 10 "http://localhost:${PORT_B}/" 2>&1)"
printf '%s' "$BODY" | grep -q "^worker " \
	&& ok "pool still serves after scale-down" \
	|| bad "request after scale-down failed (got: $(printf '%s' "$BODY" | head -c 60))"

# A retired worker parked on a channel that only closes after drop(runtime)
# keeps the runtime's blocking pool alive forever.
docker kill -s TERM scale_b >/dev/null
ELAPSED=$(wait_exit_seconds scale_b 30)
docker ps --format '{{.Names}}' | grep -q '^scale_b$' \
	&& bad "still running ${ELAPSED}s after SIGTERM — shutdown is wedged" \
	|| ok "exited ${ELAPSED}s after SIGTERM"

docker rm -f scale_b >/dev/null 2>&1

# ── Part 3: a busy worker is not offered up ──────────────────────────────────
#
# The idle stamp records when work last *arrived*, not whether it has finished,
# so a worker running a request longer than the threshold looks exactly like one
# sitting on its hands. Retiring it is not harmless: it is dropped from the
# pool's count, its slot id is handed to the next spawn, and its thread is
# joined on the pool that shutdown waits for — all while it is still serving.
#
# Traditional mode, deliberately: one request per worker at a time makes "every
# worker is busy" a fact rather than a hope, which is what lets the assertion be
# an absence. Worker mode multiplexes, so a second request there can land on the
# same thread and leave the other worker idle and retirable for honest reasons.
PORT_C="${PORT3:-$(free_port)}"
docker run -d --name scale_c \
	-e DOCUMENT_ROOT=/var/www/html \
	-e PHP_WORKERS=1:2 \
	-e PHP_WORKERS_IDLE_SECONDS=1 \
	-e LOG_LEVEL=info \
	-p "$PORT_C":80 \
	-v "$FIX:/var/www/html:ro" \
	"$IMAGE" >/dev/null || { bad "container failed to start"; exit 1; }

up=0
for _ in $(seq 1 30); do
	curl -fsS "http://localhost:${PORT_C}/ping.php" >/dev/null 2>&1 && { up=1; break; }
	sleep 1
done
[ "$up" = 1 ] \
	&& ok "container up (traditional pool)" \
	|| { bad "container never answered"; docker logs scale_c 2>&1 | tail -20; exit 1; }

END=$(( $(date +%s) + 6 ))
while [ "$(date +%s)" -lt "$END" ]; do
	curl -fsS --max-time 10 "http://localhost:${PORT_C}/ping.php" >/dev/null 2>&1
done
docker logs scale_c 2>&1 | grep -q "Scale-up: spawned worker" \
	&& ok "traditional pool grew under load" \
	|| { bad "traditional pool never grew — recalibrate before reading anything below"; docker logs scale_c 2>&1 | tail -20; exit 1; }

# Occupy both workers at once, then go quiet. Their stamps cross the 1s
# threshold while the handlers are still running.
SLOW_OUT1="$(mktemp)"; SLOW_OUT2="$(mktemp)"
curl -fsS --max-time 30 "http://localhost:${PORT_C}/slow.php?s=10" >"$SLOW_OUT1" 2>&1 &
SLOW1=$!
curl -fsS --max-time 30 "http://localhost:${PORT_C}/slow.php?s=10" >"$SLOW_OUT2" 2>&1 &
SLOW2=$!
sleep 5

docker logs scale_c 2>&1 | grep -q '"message":"Scale-down: retired worker"' \
	&& bad "retired a worker that was still serving — the stamp says idle, the worker is not" \
	|| ok "busy workers were not offered up"

# ...and the withholding is not permanent: once they finish, the same pool
# retires one. Without this, a guard that simply never retired anyone would
# pass the assertion above.
wait $SLOW1; wait $SLOW2
grep -q "^slow worker " "$SLOW_OUT1" && grep -q "^slow worker " "$SLOW_OUT2" \
	&& ok "both long requests completed" \
	|| bad "a long request did not complete (got: $(head -c 60 "$SLOW_OUT1"), $(head -c 60 "$SLOW_OUT2"))"
rm -f "$SLOW_OUT1" "$SLOW_OUT2"

retired=0
for _ in $(seq 1 40); do
	if docker logs scale_c 2>&1 | grep -q '"message":"Scale-down: retired worker"'; then retired=1; break; fi
	sleep 1
done
[ "$retired" = 1 ] \
	&& ok "pool shrank once the work was done" \
	|| { bad "pool never shrank after the work finished — the guard withholds forever"; docker logs scale_c 2>&1 | tail -10; }

echo
echo "== $PASS passed, $FAIL failed =="
[ "$FAIL" -eq 0 ]
