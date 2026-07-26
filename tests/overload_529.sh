#!/usr/bin/env bash
#
# Integration test for queue admission control.
#
# The PHP suite runner issues one request per test and waits for the response,
# so it cannot saturate a queue — nothing in it can observe a 529, because the
# 529 is the server's answer to a *different*, concurrent request. These checks
# therefore live in a standalone script that backgrounds curls, like
# tests/graceful_drain.sh.
#
# Every scenario runs with PHP_WORKERS=1 and QUEUE_CAPACITY=1, so the pool
# holds exactly one request in a worker and one in the queue; anything beyond
# that has to wait for admission.
#
#   A: a burst that fits the pool's capacity is served in full. With
#      fail-fast shedding the same burst produced 529s while the pool was
#      perfectly able to serve it — this is the regression test for that.
#   B: a pool that genuinely cannot keep up still sheds, with Retry-After,
#      after roughly the budget rather than after the request's full duration.
#   C: the permit is released when a worker picks the request up, not when it
#      finishes — otherwise a busy worker would also cost a queue slot.
#   D: QUEUE_WAIT_TIMEOUT_MS=0 restores the previous reject-immediately
#      behaviour.
#   E: QUEUE_MAX_WAITING bounds the waiting set — past it a request is refused
#      without waiting, so a sustained overload cannot park every connection.
#   F: a waiter whose client has gone gives its place in that set back instead
#      of holding it to the end of the budget.
#   G: the budget covers the wait inside the queue too. A request admitted with
#      time left over but reached long after it ran out is refused at pickup
#      rather than executed, so QUEUE_WAIT_TIMEOUT_MS bounds the whole wait and
#      not just its admission half.
#
# Handler durations are picked for discrimination, not realism: each scenario
# needs the pool to be busy for a stretch that its own budget cannot outlast
# (or, for A, comfortably can), so shortening them past what is noted below
# makes the check pass whether or not the behaviour is present.
#
# Run from run_all.sh alongside the `overflow` profile, and standalone while
# working on admission control.
#
# Usage: tests/overload_529.sh [IMAGE_REF] [--jsonl]
#   IMAGE_REF  image to test (default: oxphp-oxphp:latest)
#   --jsonl    emit one result object per check on stdout instead of a human
#              report, for run_all.sh to fold into its report
set -u

IMAGE="oxphp-oxphp:latest"
JSONL=""
for arg in "$@"; do
	case "$arg" in
		--jsonl) JSONL=1 ;;
		*) IMAGE="$arg" ;;
	esac
done
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FIX="$ROOT/tests/fixtures/overload"
PORT="${PORT:-$(python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()')}"
# Per-invocation, like $PORT: two runs of this script (a developer's and
# run_all.sh's) must not tear down each other's container mid-scenario.
SRV="overload_srv_$$"
PASS=0
FAIL=0
TMP="$(mktemp -d)"

# One JSONL object per check, matching what run_profile.sh emits so the
# standalone results land in the same report as the PHP suites.
emit() {
	python3 -c 'import json,sys; print(json.dumps({"test": sys.argv[1], "group": "admission", "pass": sys.argv[2] == "1", "assertions": [], "error": sys.argv[3], "meta": {}, "profile": "overflow"}, ensure_ascii=False))' "$1" "$2" "$3"
}
ok() {
	if [ -n "$JSONL" ]; then emit "$1" 1 ""; else printf '  \033[32mPASS\033[0m %s\n' "$1"; fi
	PASS=$((PASS + 1))
}
bad() {
	if [ -n "$JSONL" ]; then emit "$1" 0 "$1"; else printf '  \033[31mFAIL\033[0m %s\n' "$1"; fi
	FAIL=$((FAIL + 1))
}
say() { [ -n "$JSONL" ] || printf '%s\n' "$1"; }

cleanup() {
	docker rm -f "$SRV" >/dev/null 2>&1
	rm -rf "$TMP"
}
trap cleanup EXIT

start_container() {
	# start_container <queue_wait_timeout_ms> [queue_max_waiting]
	docker rm -f "$SRV" >/dev/null 2>&1
	docker run -d --name "$SRV" \
		-e DOCUMENT_ROOT=/var/www/html \
		-e PHP_WORKERS=1 \
		-e QUEUE_CAPACITY=1 \
		-e QUEUE_WAIT_TIMEOUT_MS="$1" \
		-e QUEUE_MAX_WAITING="${2:-0}" \
		-e INTERNAL_ADDR=0.0.0.0:9090 \
		-e LOG_LEVEL=error \
		-p "${PORT}":80 \
		-v "$FIX:/var/www/html:ro" \
		"$IMAGE" >/dev/null || return 1
	for _ in $(seq 1 30); do
		curl -fsS "http://localhost:${PORT}/pause.php?ms=0" >/dev/null 2>&1 && return 0
		sleep 1
	done
	return 1
}

# fire <count> <ms> <tag> — <count> concurrent requests, each holding a worker
# for <ms>. Writes "<http_code> <total_seconds>" per request to $TMP/<tag>.N
fire() {
	local count="$1" ms="$2" tag="$3" i
	for i in $(seq 1 "$count"); do
		curl -s -o /dev/null -w '%{http_code} %{time_total}\n' \
			--max-time 60 "http://localhost:${PORT}/pause.php?ms=${ms}" \
			> "$TMP/${tag}.$i" 2>&1 &
	done
	wait
}

codes()  { cat "$TMP/$1".* | awk '{print $1}'; }
count()  { codes "$1" | grep -c "^$2\$"; }

say "== queue admission control ($IMAGE) =="

# ── A: a burst inside the pool's capacity is served, not shed ────────
# 6 requests × 50 ms against one worker: capacity 1 means the last one waits
# for five pickups, ~250 ms, against a 1000 ms budget. Under fail-fast, four of
# these were 529 on arrival, so the margin is not what the check turns on — it
# is there so a loaded CI runner cannot make this look like a regression in the
# code it guards.
if start_container 1000; then
	ok "A: container up"
else
	bad "A: container failed to start"; docker logs "$SRV" 2>&1 | tail -5 >&2; exit 1
fi

fire 6 50 a
if [ "$(count a 200)" -eq 6 ]; then
	ok "A: burst within capacity fully served (6/6 → 200)"
else
	bad "A: expected 6× 200, got: $(codes a | sort | uniq -c | tr '\n' ' ')"
fi

if [ "$(docker exec "$SRV" wget -qO- http://127.0.0.1:9090/metrics 2>/dev/null \
	| grep -c '^oxphp_admission_refused_total{[^}]*} 0$')" -eq 5 ]; then
	ok "A: every oxphp_admission_refused_total reason stayed 0"
else
	bad "A: oxphp_admission_refused_total moved on a burst that was fully served"
fi

# ── B: genuine overload still sheds, by deadline ─────────────────────
# 3 s handlers against one worker: long enough that the budget expires first
# (so the shed is attributable to the deadline and not to the pool draining),
# short enough that the scenario costs seconds rather than half a minute.
# Saturate: one in the worker, one in the queue, one parked in admission.
for i in 1 2 3; do
	curl -s -o /dev/null --max-time 60 \
		"http://localhost:${PORT}/pause.php?ms=3000" >/dev/null 2>&1 &
done
sleep 0.5

# This one has nowhere to go and must be shed once its budget runs out.
read -r B_CODE B_TIME <<<"$(curl -s -o /dev/null -D "$TMP/hdr" \
	-w '%{http_code} %{time_total}' --max-time 30 \
	"http://localhost:${PORT}/pause.php?ms=3000")"

if [ "$B_CODE" = "529" ]; then
	ok "B: over-capacity load still sheds (529)"
else
	bad "B: expected 529 from a saturated pool, got $B_CODE"
fi

# The point of a wait budget: the shed arrives on its deadline (~1 s), not
# after the 3 s the blocking requests actually take. The bound has to sit
# between the two or it distinguishes nothing.
if awk -v t="$B_TIME" 'BEGIN { exit !(t < 2.5) }'; then
	ok "B: shed returned on the budget (${B_TIME}s), not after the full request"
else
	bad "B: shed took ${B_TIME}s — shedding is not deadline-driven"
fi

if grep -qi '^retry-after: 3' "$TMP/hdr"; then
	ok "B: shed carries Retry-After: 3"
else
	bad "B: shed response missing Retry-After: 3"
fi

# The reason has to be right, not just the count: an operator seeing 529s
# needs to know whether the budget expired or the waiting set filled up.
METRICS_B="$(docker exec "$SRV" wget -qO- http://127.0.0.1:9090/metrics 2>/dev/null)"
if printf '%s' "$METRICS_B" | grep -qE '^oxphp_admission_refused_total\{reason="wait_timeout"\} [1-9]'; then
	ok "B: shed counted as wait_timeout"
else
	bad "B: oxphp_admission_refused_total{reason=\"wait_timeout\"} did not move despite a shed"
fi

# The pool is still saturated right now: one request in the worker, one in the
# queue behind it. A single worker can be busy at most once — a gauge that
# counts the queue too reads 2 here, exceeds oxphp_workers_current, and drives
# oxphp_workers_idle to a saturating zero that means nothing.
B_BUSY="$(printf '%s' "$METRICS_B" | awk '/^oxphp_busy_workers /{print $2}')"
B_IDLE="$(printf '%s' "$METRICS_B" | awk '/^oxphp_workers_idle /{print $2}')"
B_PENDING="$(printf '%s' "$METRICS_B" | awk '/^oxphp_pending_requests /{print $2}')"
if [ "$B_BUSY" = "1" ] && [ "$B_IDLE" = "0" ]; then
	ok "B: busy_workers counts the worker, not the queue behind it (busy=$B_BUSY, idle=$B_IDLE)"
else
	bad "B: expected busy_workers=1 / workers_idle=0 on a one-worker pool, got busy=$B_BUSY idle=$B_IDLE"
fi
if [ "${B_PENDING:-0}" -ge 2 ]; then
	ok "B: the queued request shows up in pending_requests instead ($B_PENDING)"
else
	bad "B: expected pending_requests >= 2 with a request queued behind the worker, got $B_PENDING"
fi
wait

# ── C: the permit is released at pickup, not at completion ───────────
# Where a waiting request *sits* is the observable, not what it gets back:
# with one worker the second request is picked up when the first finishes
# either way, so its status code says nothing about the permit.
#
# Capacity 1, waiting set 1, one worker, three concurrent 2 s handlers. Held to
# completion, the executing request keeps the only queue slot, so the second
# request takes the single parking spot and the third is refused for a full
# waiting set. Released at pickup, the second request has the queue slot, the
# third parks, and nothing is refused for the cap at all.
if start_container 1000 1; then
	ok "C: container up (waiting set capped at 1)"
else
	bad "C: container failed to start"; docker logs "$SRV" 2>&1 | tail -5 >&2; exit 1
fi

# Staggered rather than fired at once: three simultaneous arrivals race the
# worker's pickup of the first, and losing that race is indistinguishable here
# from the defect under test.
for delay in 0 0.3 0.2; do
	[ "$delay" = "0" ] || sleep "$delay"
	curl -s -o /dev/null --max-time 30 "http://localhost:${PORT}/pause.php?ms=2000" &
done
wait
METRICS_C="$(docker exec "$SRV" wget -qO- http://127.0.0.1:9090/metrics 2>/dev/null)"
if printf '%s' "$METRICS_C" | grep -qE '^oxphp_admission_refused_total\{reason="waiting_full"\} 0$'; then
	ok "C: permit released at pickup — the executing request cost no queue slot"
else
	bad "C: the waiting set filled with three requests against two places — the permit is held through execution"
fi
# Positive control: without this the check above also passes when the three
# requests never overlapped and nothing had to wait for anything.
C_WAITED="$(printf '%s' "$METRICS_C" | awk '/^oxphp_admission_refused_total\{reason="wait_timeout"\}/{print $2}')"
if [ "${C_WAITED:-0}" -ge 2 ]; then
	ok "C: both the queued and the parked request did have to wait ($C_WAITED)"
else
	bad "C: only ${C_WAITED:-0} request waited — the pool was not saturated and the check above proved nothing"
fi

# ── D: QUEUE_WAIT_TIMEOUT_MS=0 restores fail-fast ────────────────────
if start_container 0; then
	ok "D: container up (fail-fast mode)"
else
	bad "D: container failed to start"; docker logs "$SRV" 2>&1 | tail -5 >&2; exit 1
fi

fire 6 100 d
SHED_D="$(count d 529)"
if [ "$SHED_D" -ge 1 ]; then
	ok "D: fail-fast sheds the same burst scenario A served ($SHED_D × 529)"
else
	bad "D: expected 529s with QUEUE_WAIT_TIMEOUT_MS=0, got: $(codes d | sort | uniq -c | tr '\n' ' ')"
fi

FAST_SHED="$(awk '$1 == 529 && $2 < 1 {n++} END {print n + 0}' "$TMP"/d.*)"
if [ "$FAST_SHED" -eq "$SHED_D" ]; then
	ok "D: every fail-fast shed returned in under a second"
else
	bad "D: only $FAST_SHED of $SHED_D sheds were immediate — the budget is still being applied"
fi

if docker exec "$SRV" wget -qO- http://127.0.0.1:9090/metrics 2>/dev/null \
	| grep -qE '^oxphp_admission_refused_total\{reason="queue_full"\} [1-9]'; then
	ok "D: fail-fast shed counted as queue_full, not wait_timeout"
else
	bad "D: fail-fast shed was not counted under reason=\"queue_full\""
fi

# ── E: QUEUE_MAX_WAITING bounds the waiting set ──────────────────────
# The cap is what keeps a sustained overload from parking every connection
# until the accept loop stalls, so its refusal has to be immediate — a shed
# that still costs a full budget of waiting is not a cap.
#
# QUEUE_MAX_WAITING=1 with capacity 1 and one worker: of four concurrent 2 s
# requests, one runs, one holds the queue slot, one takes the single parking
# spot, and the fourth has nowhere to go at all.
if start_container 1000 1; then
	ok "E: container up (waiting set capped at 1)"
else
	bad "E: container failed to start"; docker logs "$SRV" 2>&1 | tail -5 >&2; exit 1
fi

fire 4 2000 e
if [ "$(count e 529)" -ge 1 ]; then
	ok "E: capped waiting set sheds ($(count e 529) × 529)"
else
	bad "E: expected at least one 529, got: $(codes e | sort | uniq -c | tr '\n' ' ')"
fi

# The distinguishing property: refusal past the cap does not wait. A shed at
# ~1 s is the budget expiring, which is the other reason and the other knob.
CAP_SHED="$(awk '$1 == 529 && $2 < 0.5 {n++} END {print n + 0}' "$TMP"/e.*)"
if [ "$CAP_SHED" -ge 1 ]; then
	ok "E: shed past the cap returned immediately, without spending the budget"
else
	bad "E: every 529 took at least 0.5s — the cap is not refusing, the budget is expiring"
fi

if docker exec "$SRV" wget -qO- http://127.0.0.1:9090/metrics 2>/dev/null \
	| grep -qE '^oxphp_admission_refused_total\{reason="waiting_full"\} [1-9]'; then
	ok "E: shed counted as waiting_full, not wait_timeout"
else
	bad "E: oxphp_admission_refused_total{reason=\"waiting_full\"} did not move despite a capped shed"
fi

# ── F: a departed client gives its place in the waiting set back ─────
# The place is a hard gate — past it requests are refused outright — so a
# waiter that keeps its place after its client is gone spends the scarcest
# resource admission has on nobody. Under a balancer that times out and
# retries, that is the common case, not the edge one: the set fills with
# attempts the balancer has already abandoned and the retries it sent instead
# are the ones refused.
#
# Nothing in the admission code implements this: the place comes back because
# hyper drops the request future, which drops the wait, which releases the
# permit. The property is worth pinning down precisely because no code owns it
# — awaiting the wait inside the connection task instead of a detached one is
# the only thing holding it up.
#
# HTTP/2 for the abandoning client, because that is where the departure is
# visible at all. An HTTP/1.1 close mid-request never reaches the server.
if start_container 5000 1; then
	ok "F: container up (5s budget, waiting set capped at 1)"
else
	bad "F: container failed to start"; docker logs "$SRV" 2>&1 | tail -5 >&2; exit 1
fi

# r1 takes the worker for 4 s, r2 the queue slot (it is short, but it holds the
# slot until the worker is free at 4 s), r3 the single parking spot — then r3
# gives up at 0.8 s, a good four seconds before its budget would have expired.
curl -s -o /dev/null --max-time 40 "http://localhost:${PORT}/pause.php?ms=4000" &
sleep 0.3
curl -s -o /dev/null --max-time 40 "http://localhost:${PORT}/pause.php?ms=100" &
sleep 0.3
curl -s -o /dev/null --http2-prior-knowledge --max-time 0.8 \
	"http://localhost:${PORT}/pause.php?ms=4000" >/dev/null 2>&1 &
R3_PID=$!

# Negative control. Both checks below pass vacuously if r3 never reached the
# waiting set at all — a runner that shifted the timing, or an h2c handshake
# that did not happen, leaves the spot free for reasons that have nothing to do
# with releasing it. Pin r3 down while it is still parked: one request in the
# worker, one in the queue, one waiting, and none of them answered yet.
sleep 0.4
F_PENDING="$(docker exec "$SRV" wget -qO- http://127.0.0.1:9090/metrics 2>/dev/null \
	| awk '/^oxphp_pending_requests /{print $2}')"
if [ "${F_PENDING:-0}" -eq 3 ]; then
	ok "F: r3 really was parked in the waiting set before its client left"
else
	bad "F: expected 3 requests in flight with r3 parked, got ${F_PENDING:-0} — the rest of F proves nothing"
fi

# Well after r3's client is gone, well before r3's budget would have run out,
# and while the queue slot is still held — so this request has to park, and
# the only spot is the one r3 is no longer using.
sleep 0.7
F_CODE="$(curl -s -o /dev/null -w '%{http_code}' --max-time 40 \
	"http://localhost:${PORT}/pause.php?ms=100")"
wait "$R3_PID"; R3_RC=$?
wait

# 28 is curl's own timeout: r3 was still waiting for a response when its client
# walked away. Any other code means it was answered — a shed, or a connection
# that never got established — and it was never holding a place to give back.
if [ "$R3_RC" -eq 28 ]; then
	ok "F: r3's client left mid-wait, unanswered (curl 28)"
else
	bad "F: r3 exited $R3_RC, not 28 — it was answered rather than abandoned mid-wait"
fi

if [ "$F_CODE" = "200" ]; then
	ok "F: the place a departed client left was reusable"
else
	bad "F: the request that had to park got $F_CODE — a client long gone still holds the spot"
fi

if docker exec "$SRV" wget -qO- http://127.0.0.1:9090/metrics 2>/dev/null \
	| grep -qE '^oxphp_admission_refused_total\{reason="waiting_full"\} 0$'; then
	ok "F: nothing was refused for a waiting set that was not really full"
else
	bad "F: oxphp_admission_refused_total{reason=\"waiting_full\"} moved — the abandoned wait was still occupying the cap"
fi

# ── G: the budget covers the wait inside the queue as well ───────────
# Capacity 1, one worker, a 1 s budget and a 3 s handler. The second request is
# admitted immediately — there is a free queue slot the moment the first is
# picked up — so admission never refuses it. It is then reached three seconds
# later, two seconds past a budget the operator set to one.
#
# The timing is the whole check: a 529 at ~1 s is the admission gate, which
# this scenario deliberately does not exercise. A 529 at ~3 s can only come
# from the pickup check, and a 200 at ~3.1 s means the worker ran a request
# whose deadline had passed.
if start_container 1000; then
	ok "G: container up (1 s budget)"
else
	bad "G: container failed to start"; docker logs "$SRV" 2>&1 | tail -5 >&2; exit 1
fi

curl -s -o /dev/null --max-time 40 "http://localhost:${PORT}/pause.php?ms=3000" &
sleep 0.2
read -r G_CODE G_TIME <<<"$(curl -s -o /dev/null -D "$TMP/ghdr" \
	-w '%{http_code} %{time_total}' --max-time 40 \
	"http://localhost:${PORT}/pause.php?ms=100")"
wait

if [ "$G_CODE" = "529" ]; then
	ok "G: a request queued past its budget is refused, not executed"
else
	bad "G: expected 529, got $G_CODE — the budget bounds admission only, and the queue wait is unbounded"
fi

if awk -v t="$G_TIME" 'BEGIN { exit !(t > 2.5) }'; then
	ok "G: refused at pickup (${G_TIME}s), so admission had let it through"
else
	bad "G: answered in ${G_TIME}s — that is the admission gate, not the queue"
fi

if grep -qi '^retry-after: 3' "$TMP/ghdr"; then
	ok "G: the pickup refusal is the same shed the gate emits (Retry-After: 3)"
else
	bad "G: pickup refusal missing Retry-After: 3"
fi

if docker exec "$SRV" wget -qO- http://127.0.0.1:9090/metrics 2>/dev/null \
	| grep -qE '^oxphp_admission_refused_total\{reason="wait_timeout"\} [1-9]'; then
	ok "G: counted as wait_timeout, in the same series as the gate's own"
else
	bad "G: the pickup refusal was not counted under reason=\"wait_timeout\""
fi

say ""
say "passed: $PASS, failed: $FAIL"
[ "$FAIL" -eq 0 ]
