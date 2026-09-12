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
# Every scenario runs with PHP_WORKERS=1, and all but M2 with QUEUE_CAPACITY=1,
# so the pool holds exactly one request in a worker and one in the queue and
# anything beyond that has to wait for admission. M2 is the exception on
# purpose: the behaviour it is about only exists where a queue slot is free,
# which at the default capacity is almost always.
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
#      time left over is refused when that time runs out rather than whenever a
#      worker next becomes free, so QUEUE_WAIT_TIMEOUT_MS bounds the whole wait
#      and not just its admission half.
#   H: a queue sized to hold every connection the server may accept is reported
#      at startup and by `config --check`, instead of being found under load.
#   I: F over HTTP/1.1 — a client that closes mid-wait is seen on that protocol
#      too, so its place comes back and its script is never run.
#   J: the waiting set is bounded in bytes as well as in places — a request
#      whose buffered body would push the parked bodies past
#      QUEUE_MAX_WAITING_BYTES is refused on the spot, while smaller ones go on
#      waiting out their budget.
#   K: an exhausted connection budget is visible while it lasts — the parked
#      accept loop logs its entry and its exit and moves a gauge and a
#      counter, instead of the silence that left the state indistinguishable
#      from a dead node.
#   L: the rate limit on those reports does not outlive the stall it
#      suppresses — a stall beginning right after a reported one is still
#      reported, once the window closes, rather than staying silent for as
#      long as it lasts.
#   M: an application that calls back into this same server over HTTP. The
#      inner call can only be served once the outer one frees its worker, and
#      the outer one is waiting for the inner one, so the wait cannot succeed.
#      Two shapes, because which one a deployment gets is decided by
#      QUEUE_CAPACITY: with no free slot the inner call waits at the gate and
#      is refused on its deadline (M1, the budget spent for nothing), and with
#      a slot free it is admitted to the queue instead and waits there (M2,
#      where the budget has to be enforced by something other than a pickup
#      that is never coming). M3 is the control the deadline must not catch: a
#      request a worker did pick up in time runs past the budget and is served.
#   N: the negative control for M. The same refusal, on a pool that is working
#      its way through the queue the whole time a request waits: the wait fails
#      but it failed a race, and the wasted-wait series must stay still for it.
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
	# start_container <queue_wait_timeout_ms> [queue_max_waiting] [queue_max_waiting_bytes]
	docker rm -f "$SRV" >/dev/null 2>&1
	docker run -d --name "$SRV" \
		-e DOCUMENT_ROOT=/var/www/html \
		-e PHP_WORKERS=1 \
		-e QUEUE_CAPACITY=1 \
		-e QUEUE_WAIT_TIMEOUT_MS="$1" \
		-e QUEUE_MAX_WAITING="${2:-0}" \
		-e QUEUE_MAX_WAITING_BYTES="${3:-0}" \
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

# Accepted PHP requests not yet answered: the ones in a worker, the ones queued
# behind it, and the ones parked in admission. A failed scrape prints -1, which
# is a count no check can expect: an empty string would make `[ -eq ]` a syntax
# error, and a 0 would quietly satisfy any check whose healthy value is "nothing
# left in flight".
pending() {
	docker exec "$SRV" wget -qO- http://127.0.0.1:9090/metrics 2>/dev/null \
		| awk '/^oxphp_pending_requests /{print $2; found=1} END{if (!found) print -1}'
}

# A failed scrape prints -1 rather than an empty string: an arithmetic test
# against "" is a syntax error, and one against 0 would quietly pass every
# check whose healthy value is "nothing yet". The name is matched as a whole
# field rather than as a substring, so the `# HELP` line carrying the same name
# does not turn the answer into two lines.
gauge() {
	docker exec "$SRV" wget -qO- http://127.0.0.1:9090/metrics 2>/dev/null \
		| awk -v k="$1" '$1 == k {print $2; found=1} END{if (!found) print -1}'
}

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
	| grep -c '^oxphp_admission_refused_total{[^}]*} 0$')" -eq 6 ]; then
	ok "A: every oxphp_admission_refused_total reason stayed 0"
else
	bad "A: oxphp_admission_refused_total moved on a burst that was fully served"
fi

# The negative control for the wasted-wait series. Waits happened here — the
# last of the six queued behind five pickups — and every one of them ended in
# a worker. A counter that moved on those would be counting waiting, not
# waiting for nothing, and would read as a fault on every healthy burst.
A_WASTED="$(gauge 'oxphp_admission_wait_wasted_total')"
if [ "$A_WASTED" = "0" ]; then
	ok "A: oxphp_admission_wait_wasted_total stayed 0 — waits that succeed are not wasted"
else
	bad "A: oxphp_admission_wait_wasted_total reads ${A_WASTED} (-1 = the series is not exported at all) — on this burst every wait ended in a worker and none of them was wasted"
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

# Read the saturation gauges *now*, while the pool is actually saturated: one
# request in the worker, one in the queue behind it, one parked at the gate.
# Half a second in, none of the three has reached its 1 s budget yet. Later is
# too late — the two that are waiting are answered on that budget, so a scrape
# taken after the shed below finds only the executing request and reads the
# absence of a queue as a gauge that does not count it.
METRICS_B_SAT="$(docker exec "$SRV" wget -qO- http://127.0.0.1:9090/metrics 2>/dev/null)"

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

# From the saturation scrape taken above, not this one. A single worker can be
# busy at most once — a gauge that counts the queue too reads 2 there, exceeds
# oxphp_workers_current, and drives oxphp_workers_idle to a saturating zero
# that means nothing.
B_BUSY="$(printf '%s' "$METRICS_B_SAT" | awk '/^oxphp_busy_workers /{print $2}')"
B_IDLE="$(printf '%s' "$METRICS_B_SAT" | awk '/^oxphp_workers_idle /{print $2}')"
B_PENDING="$(printf '%s' "$METRICS_B_SAT" | awk '/^oxphp_pending_requests /{print $2}')"
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
# HTTP/2 for the abandoning client here; scenario I runs the same timeline over
# HTTP/1.1, where the departure arrives as an EOF on the socket hyper is still
# reading.
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
# picked up — so admission never refuses it. Its budget then runs out while it
# sits in the queue, two full seconds before the worker is free to look at it.
#
# What distinguishes this from the admission gate is the queue, not the clock.
# Both refusals now land at about the budget, so the check that this scenario
# is about the *second* wait is that the request was admitted: a queued request
# and no slot left to admit another. Reading it from the timing instead —
# "later than the gate could have answered" — is what the old version did, and
# it only worked while the queue wait was the one thing the budget failed to
# bound.
if start_container 1000; then
	ok "G: container up (1 s budget)"
else
	bad "G: container failed to start"; docker logs "$SRV" 2>&1 | tail -5 >&2; exit 1
fi

curl -s -o /dev/null --max-time 40 "http://localhost:${PORT}/pause.php?ms=3000" &
sleep 0.2
curl -s -o /dev/null -D "$TMP/ghdr" -w '%{http_code} %{time_total}' --max-time 40 \
	"http://localhost:${PORT}/pause.php?ms=100" > "$TMP/g.res" &
G_PID=$!
# While it waits: taken off admission and sitting in the channel. Read before
# anything that waits on the request itself, or the window has closed.
sleep 0.4
G_DEPTH="$(gauge 'oxphp_queue_depth')"
G_SLOTS="$(gauge 'oxphp_admission_slots_available')"
wait "$G_PID"
read -r G_CODE G_TIME < "$TMP/g.res"
wait

if [ "${G_DEPTH:--1}" -ge 1 ] && [ "${G_SLOTS:--1}" = "0" ]; then
	ok "G: the request was admitted and waiting in the queue (depth ${G_DEPTH}, no slot left)"
else
	bad "G: queue depth ${G_DEPTH:-?} with ${G_SLOTS:-?} slots free — the request never reached the queue, so the checks below are about the gate"
fi

if [ "$G_CODE" = "529" ]; then
	ok "G: a request queued past its budget is refused, not executed"
else
	bad "G: expected 529, got $G_CODE — the budget bounds admission only, and the queue wait is unbounded"
fi

if awk -v t="$G_TIME" 'BEGIN { exit !(t > 0.8 && t < 2.5) }'; then
	ok "G: refused on its budget (${G_TIME}s), not when the worker got round to it"
else
	bad "G: answered in ${G_TIME}s against a 1 s budget — past 2.5 s it is waiting for the pickup rather than for the deadline"
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

# ── H: a queue sized to hold every connection says so at startup ─────
# The queue, the waiting set and the workers each hold a connection until
# their request is answered, so once they add up to MAX_CONNECTIONS the accept
# loop parks and clients get no answer at all — worse than the 529 this whole
# file is about, and previously silent. The check is diagnostic, so the only
# thing to assert is the diagnosis: present when the sum reaches the budget,
# absent when it does not. Both containers use the same knobs and differ only
# in MAX_CONNECTIONS, so nothing but the comparison can explain the difference.
WARN_RE='the PHP path alone can hold every allowed connection'

# The warning is emitted while the configuration is parsed, long before the
# listener is up, so a server answering a request has certainly emitted it if it
# was going to. Waiting for that rather than for a fixed interval is what keeps
# the absence half from passing on a runner that was merely slow.
start_sized_container() {
	# start_sized_container <max_connections>
	docker rm -f "$SRV" >/dev/null 2>&1
	docker run -d --name "$SRV" \
		-e DOCUMENT_ROOT=/var/www/html -e PHP_WORKERS=1 \
		-e QUEUE_CAPACITY=4 -e QUEUE_MAX_WAITING=4 -e MAX_CONNECTIONS="$1" \
		-e LOG_LEVEL=info -p "${PORT}":80 -v "$FIX:/var/www/html:ro" \
		"$IMAGE" >/dev/null || return 1
	for _ in $(seq 1 30); do
		curl -fsS "http://localhost:${PORT}/pause.php?ms=0" >/dev/null 2>&1 && return 0
		sleep 1
	done
	return 1
}

if start_sized_container 8; then
	if docker logs "$SRV" 2>&1 | grep -q "$WARN_RE"; then
		ok "H: a PHP path sized to take every connection warns at startup"
	else
		bad "H: 1 worker + 4 queued + 4 parked against MAX_CONNECTIONS=8 went unreported"
	fi
	if docker exec -e PHP_WORKERS=1 -e QUEUE_CAPACITY=4 -e QUEUE_MAX_WAITING=4 \
		-e MAX_CONNECTIONS=8 -e DOCUMENT_ROOT=/var/www/html \
		"$SRV" oxphp config --check 2>&1 | grep -q '^  ! PHP_WORKERS'; then
		ok "H: config --check reports it too, where the startup log is not yet running"
	else
		bad "H: config --check stayed silent about a queue that can take every connection"
	fi
else
	bad "H: container failed to start"
fi

if start_sized_container 64; then
	if docker logs "$SRV" 2>&1 | grep -q "$WARN_RE"; then
		bad "H: the same queue under a connection budget with room to spare still warned"
	else
		ok "H: room to spare in the connection budget stays quiet"
	fi
else
	bad "H: control container failed to start"
fi

# ── I: the same departure over HTTP/1.1 ──────────────────────────────
# F rides HTTP/2, where hyper surfaces the departure through the connection
# future. HTTP/1.1 was long assumed to report nothing at all while a handler
# is running, which would make an abandoned wait hold the scarcest resource
# admission has for the rest of its budget — and would make the whole waiting
# set fill with attempts a timing-out balancer has already given up on.
#
# It does report it: with no response written yet, hyper is reading the socket
# for exactly this, and an EOF mid-message ends the connection and drops the
# request future with it. Nothing in this repository implements that, which is
# why it is pinned here: the property is inherited, and an upgrade or a stray
# `half_close(true)` would remove it silently.
#
# Same shape as F, one worker, budget 5 s, one parking spot, every client
# pinned to HTTP/1.1.
if start_container 5000 1; then
	ok "I: container up (5s budget, waiting set capped at 1)"
else
	bad "I: container failed to start"; docker logs "$SRV" 2>&1 | tail -5 >&2; exit 1
fi

curl -s -o /dev/null --http1.1 --max-time 40 "http://localhost:${PORT}/pause.php?ms=4000" &
sleep 0.3
curl -s -o /dev/null --http1.1 --max-time 40 "http://localhost:${PORT}/pause.php?ms=100" &
sleep 0.3
curl -s -o /dev/null --http1.1 --max-time 0.8 \
	"http://localhost:${PORT}/pause.php?ms=4000" >/dev/null 2>&1 &
I_PID=$!

# Negative control, as in F: one in the worker, one in the queue, one parked and
# none of them answered. Without it every check below passes on a run where the
# third request never reached the waiting set at all.
sleep 0.4
I_PARKED="$(pending)"
if [ "${I_PARKED:-0}" -eq 3 ]; then
	ok "I: the h1 client really was parked in the waiting set before it left"
else
	bad "I: expected 3 requests in flight with one parked, got ${I_PARKED:-0} — the rest of I proves nothing"
fi

# Its client is gone by now, a good four seconds before the budget would have
# expired. The same gauge that read 3 above has to have dropped.
sleep 0.7
I_AFTER="$(pending)"
if [ "${I_AFTER:-9}" -eq 2 ]; then
	ok "I: the departed h1 waiter stopped counting as in flight"
else
	bad "I: still ${I_AFTER:-?} in flight after the h1 client left — the wait outlived it"
fi

# And the freed spot is usable: the queue slot is still held, so this one has to
# park, and the only place is the one the departed client is no longer using.
# Backgrounded, because the gauge below has to be read at a moment this file
# picks and not one the server does: how long this request takes is itself a
# symptom — milliseconds if it is refused for a place still held by the departed
# client, seconds if it is served — so anchoring the reading to it would sample
# the gauge at whatever moment the behaviour under test produced.
curl -s -o /dev/null -w '%{http_code}' --http1.1 --max-time 40 \
	"http://localhost:${PORT}/pause.php?ms=100" > "$TMP/i.late" 2>&1 &

# t ≈ 5.2 s: the worker freed up at 4 s and everything genuinely in flight has
# been answered, while a 4 s script started on that free worker would still be
# running until ~8 s. The window is what makes the reading mean something.
sleep 3.5
I_LATE="$(pending)"
wait "$I_PID"; I_RC=$?
wait
I_CODE="$(cat "$TMP/i.late")"

if [ "$I_RC" -eq 28 ]; then
	ok "I: the h1 client left mid-wait, unanswered (curl 28)"
else
	bad "I: the abandoning request exited $I_RC, not 28 — it was answered rather than abandoned mid-wait"
fi

if [ "$I_CODE" = "200" ]; then
	ok "I: the place a departed h1 client left was reusable"
else
	bad "I: the request that had to park got $I_CODE — an h1 client long gone still holds the spot"
fi

if [ "${I_LATE:-9}" -eq 0 ]; then
	ok "I: the departed client's script was never run"
else
	bad "I: ${I_LATE:-?} still in flight — a worker is executing PHP for a client that is gone"
fi

# The other shape the same defect takes: the departed waiter keeps its place to
# the end of the budget and is then shed, which leaves the gauge at zero too but
# moves a counter. Either reason moving means the wait outlived its client.
# Counting the two zero lines rather than grepping for a non-zero one: a scrape
# that returns nothing at all matches no non-zero line either, and would read as
# "nothing was refused".
I_ZEROS="$(docker exec "$SRV" wget -qO- http://127.0.0.1:9090/metrics 2>/dev/null \
	| grep -cE '^oxphp_admission_refused_total\{reason="(waiting_full|wait_timeout)"\} 0$')"
if [ "${I_ZEROS:-0}" -eq 2 ]; then
	ok "I: nothing was refused for a place or a budget spent on a departed client"
else
	bad "I: a refusal was counted — the abandoned h1 wait was still occupying admission"
fi

# ── J: the waiting set is bounded in bytes, not only in places ───────
# A parked request holds its request body, fully buffered, for as long as it
# waits. Places alone bound that in requests and not in memory, so a waiting set
# of a few hundred can hold gigabytes of bodies for the whole budget — and the
# only thing an operator could tune was how many requests wait, which says
# nothing about how large they are.
#
# One worker, capacity 1, three parking places and a 64 KiB byte budget. Two 5 s
# handlers take the worker and the queue slot, so everything after them has to
# park — long enough that no waiter here can be admitted by the pool draining
# instead of by the behaviour under test. Three places for three arrivals: the
# count cap cannot be what refuses anything, which is what makes the byte budget
# the only available explanation for a refusal.
#
# Both halves are checked. A 1 MiB body is refused on the spot — and the empty
# body and the 1 KiB body still park and still wait their budget out, so
# "refuse anything with a body" and "stop waiting altogether" both fail this.
#
# Then the aggregate, which the three above cannot see: two 40 KiB bodies, each
# well inside the budget and over it together. The first must park and wait,
# the second must be refused for the bytes the first is holding. This is the
# only check here that fails if the charge does not outlive the wait — release
# it early and every other check in this file still passes, because a body
# larger than the whole budget is refused against an empty counter either way.
if start_container 1000 3 65536; then
	ok "J: container up (1 s budget, 3 places, 64 KiB of parked bodies)"
else
	bad "J: container failed to start"; docker logs "$SRV" 2>&1 | tail -5 >&2; exit 1
fi

python3 -c 'import sys; sys.stdout.write("x" * 1048576)' > "$TMP/big.bin"
python3 -c 'import sys; sys.stdout.write("x" * 1024)' > "$TMP/small.bin"
# 40 KiB each: either one parks inside a 64 KiB budget, the two together do not.
python3 -c 'import sys; sys.stdout.write("x" * 40960)' > "$TMP/half.bin"

# `Expect:` off — curl asks for 100-continue on bodies this size, and the round
# trip would land in the timings the checks below turn on.
post() {
	# post <file> <tag>
	curl -s -o /dev/null -w '%{http_code} %{time_total}\n' --max-time 30 \
		-H 'Expect:' -H 'Content-Type: application/octet-stream' \
		--data-binary "@$1" "http://localhost:${PORT}/pause.php?ms=100" \
		> "$TMP/$2" 2>&1
}

curl -s -o /dev/null --max-time 40 "http://localhost:${PORT}/pause.php?ms=5000" &
sleep 0.3
curl -s -o /dev/null --max-time 40 "http://localhost:${PORT}/pause.php?ms=5000" &
sleep 0.3

post "$TMP/big.bin" j.big
read -r J_BIG_CODE J_BIG_TIME < "$TMP/j.big"

# The two that must still wait, in parallel: both park, both outlive nothing.
post "$TMP/small.bin" j.small &
J_SMALL_PID=$!
curl -s -o /dev/null -w '%{http_code} %{time_total}\n' --max-time 30 \
	"http://localhost:${PORT}/pause.php?ms=100" > "$TMP/j.empty" 2>&1 &
J_EMPTY_PID=$!
wait "$J_SMALL_PID" "$J_EMPTY_PID"
read -r J_SMALL_CODE J_SMALL_TIME < "$TMP/j.small"
read -r J_EMPTY_CODE J_EMPTY_TIME < "$TMP/j.empty"

# The aggregate. The first 40 KiB body parks and is still holding its charge
# when the second arrives; 40 + 40 is past 64, so the second has nowhere to sit.
# Sequenced rather than fired together, because which of the two is refused is
# the whole point and a race would decide it arbitrarily.
post "$TMP/half.bin" j.half1 &
J_HALF1_PID=$!
sleep 0.3
post "$TMP/half.bin" j.half2
read -r J_HALF2_CODE J_HALF2_TIME < "$TMP/j.half2"
wait "$J_HALF1_PID"
read -r J_HALF1_CODE J_HALF1_TIME < "$TMP/j.half1"

METRICS_J="$(docker exec "$SRV" wget -qO- http://127.0.0.1:9090/metrics 2>/dev/null)"
wait

if [ "$J_BIG_CODE" = "529" ] && awk -v t="$J_BIG_TIME" 'BEGIN { exit !(t < 0.5) }'; then
	ok "J: a body past the byte budget is refused on the spot (${J_BIG_TIME}s)"
else
	bad "J: expected an immediate 529 for a 1 MiB body, got $J_BIG_CODE after ${J_BIG_TIME}s — it parked and held its body instead"
fi

if printf '%s' "$METRICS_J" | grep -qE '^oxphp_admission_refused_total\{reason="waiting_bytes"\} [1-9]'; then
	ok "J: refused for the byte budget, and counted as its own reason"
else
	bad "J: oxphp_admission_refused_total{reason=\"waiting_bytes\"} did not move — an operator cannot tell which cap refused this"
fi

# The other half. Without these two, a build that simply stopped waiting would
# pass everything above.
if [ "$J_EMPTY_CODE" = "529" ] && awk -v t="$J_EMPTY_TIME" 'BEGIN { exit !(t > 0.9) }'; then
	ok "J: a bodyless request still parks and waits its budget out (${J_EMPTY_TIME}s)"
else
	bad "J: the bodyless request answered $J_EMPTY_CODE after ${J_EMPTY_TIME}s — the byte budget refuses requests that hold nothing"
fi

if [ "$J_SMALL_CODE" = "529" ] && awk -v t="$J_SMALL_TIME" 'BEGIN { exit !(t > 0.9) }'; then
	ok "J: a body inside the budget waits like any other request (${J_SMALL_TIME}s)"
else
	bad "J: the 1 KiB body answered $J_SMALL_CODE after ${J_SMALL_TIME}s — carrying a body at all is what lost the wait, not its size"
fi

J_WAITED_OUT="$(printf '%s' "$METRICS_J" \
	| awk '/^oxphp_admission_refused_total\{reason="wait_timeout"\} /{print $2}')"
if [ "${J_WAITED_OUT:-0}" -ge 2 ]; then
	ok "J: the two that waited were refused by the budget, not by the byte cap"
else
	bad "J: oxphp_admission_refused_total{reason=\"wait_timeout\"} reads ${J_WAITED_OUT:-absent} — expected at least the two that waited, alongside the byte-budget one"
fi

# The charge outlives the wait, or it bounds nothing. Neither body is refusable
# on its own here, so only the first one's charge still being held can explain
# the second one's refusal.
if [ "$J_HALF1_CODE" = "529" ] && awk -v t="$J_HALF1_TIME" 'BEGIN { exit !(t > 0.9) }'; then
	ok "J: the first 40 KiB body parked and waited its budget out (${J_HALF1_TIME}s)"
else
	bad "J: the first 40 KiB body answered $J_HALF1_CODE after ${J_HALF1_TIME}s — it never parked, so the next check proves nothing"
fi

if [ "$J_HALF2_CODE" = "529" ] && awk -v t="$J_HALF2_TIME" 'BEGIN { exit !(t < 0.5) }'; then
	ok "J: bodies are charged in aggregate — 40 KiB + 40 KiB does not fit 64 KiB (${J_HALF2_TIME}s)"
else
	bad "J: the second 40 KiB body answered $J_HALF2_CODE after ${J_HALF2_TIME}s — the first one's charge was not held for its wait, so the budget bounds one body rather than the parked set"
fi

# ── K: an exhausted connection budget is visible while it lasts ──────
# Once the PHP path holds every MAX_CONNECTIONS permit, the accept loop parks
# with a connection already accepted and nothing being served — the state H
# warns about at startup, reached at runtime. Parking is the designed
# behaviour (a parked loop spends nothing on load it cannot serve, and the
# listen backlog keeps late clients queued); what is under test is that the
# state is visible while it lasts: a WARN when the loop first has to wait, a
# gauge an alert can read without knowing the budget, a counter that survives
# the scrape interval, and an INFO when accepting resumes. The probe that
# gets no answer is the precondition, not the defect — it pins that the loop
# really was parked when the log and the gauge said so, and that visibility
# did not quietly change parking into refusal.
#
# 1 worker + 4 queue slots + 4 parking places against MAX_CONNECTIONS=8: nine
# 4 s requests fill all three populations and the ninth connection takes the
# loop past the budget. LOG_LEVEL=info because the resume line is the INFO
# half of the pair under test.
docker rm -f "$SRV" >/dev/null 2>&1
docker run -d --name "$SRV" \
	-e DOCUMENT_ROOT=/var/www/html -e PHP_WORKERS=1 \
	-e QUEUE_CAPACITY=4 -e QUEUE_MAX_WAITING=4 -e QUEUE_WAIT_TIMEOUT_MS=6000 \
	-e MAX_CONNECTIONS=8 -e INTERNAL_ADDR=0.0.0.0:9090 -e LOG_LEVEL=info \
	-p "${PORT}":80 -v "$FIX:/var/www/html:ro" \
	"$IMAGE" >/dev/null
K_UP=""
for _ in $(seq 1 30); do
	curl -fsS "http://localhost:${PORT}/pause.php?ms=0" >/dev/null 2>&1 && { K_UP=1; break; }
	sleep 1
done
if [ -n "$K_UP" ]; then
	ok "K: container up (MAX_CONNECTIONS=8 against 1 worker + 4 queued + 4 parked)"
else
	bad "K: container failed to start"; docker logs "$SRV" 2>&1 | tail -5 >&2; exit 1
fi

for _ in $(seq 1 9); do
	curl -s -o /dev/null --max-time 40 \
		"http://localhost:${PORT}/pause.php?ms=4000" >/dev/null 2>&1 &
done

# Negative control: every check below is vacuous unless the budget really is
# exhausted. 8 dispatched requests (one running, four queued, three parked in
# admission) is the whole budget; the ninth connection is then in the accept
# loop's hands, waiting for a permit that does not exist.
K_PENDING=-1
for _ in $(seq 1 20); do
	K_PENDING="$(pending)"
	[ "$K_PENDING" = "8" ] && break
	sleep 0.25
done
if [ "$K_PENDING" = "8" ]; then
	ok "K: every permit is spoken for (pending=8)"
else
	bad "K: expected 8 dispatched requests, got $K_PENDING — the budget was never exhausted and the rest of K proves nothing"
fi

# The gauge, read through the internal listener — which serves during the
# stall precisely because it does not go through MAX_CONNECTIONS. Retried
# briefly: the ninth connection has to reach the loop before the gauge moves.
K_STALLED=""
for _ in $(seq 1 8); do
	if docker exec "$SRV" wget -qO- http://127.0.0.1:9090/metrics 2>/dev/null \
		| grep -q '^oxphp_accept_stalled 1$'; then K_STALLED=1; break; fi
	sleep 0.25
done
if [ -n "$K_STALLED" ]; then
	ok "K: oxphp_accept_stalled reads 1 while the loop is parked"
else
	bad "K: oxphp_accept_stalled never read 1 during the stall"
fi

if docker logs "$SRV" 2>&1 | grep -q 'accept loop parked'; then
	ok "K: the stall is in the log the moment it starts"
else
	bad "K: the loop parked without a line in the log"
fi

# Parking stayed parking: a client arriving now gets no answer at all, and
# times out on its own — visibility must not have turned the stall into a
# refusal. Nothing can answer it inside its window: no handler finishes for
# another two seconds, and the permit that first one does release goes to the
# connection already parked in the loop — the only waiter on the semaphore — so
# this probe is either still in the kernel's backlog or accepted and parked in
# its turn, and unanswered on both.
K_CODE="$(curl -s -o /dev/null -w '%{http_code}' --max-time 2 \
	"http://localhost:${PORT}/pause.php?ms=0")"
K_RC=$?
if [ "$K_RC" -eq 28 ] && [ "$K_CODE" = "000" ]; then
	ok "K: a client past the budget still gets no answer, not a refusal (curl 28)"
else
	bad "K: the probe got code $K_CODE rc $K_RC — the loop was not parked, or parking became something else"
fi

wait

# Checked before anything else is sent, which is the whole point: an overload
# ends because the load went away, so there may be no next connection for
# minutes — or none before the instance is stopped. A resume line written by
# the next accept instead of by the end of the stall would date the recovery
# to whenever traffic happened to return, and a post-mortem on a quiet
# instance would read "parked until it died".
if docker logs "$SRV" 2>&1 | grep -q 'accept loop resumed'; then
	ok "K: the exit from the stall is in the log without waiting for more traffic"
else
	bad "K: the loop resumed accepting without a line in the log"
fi

K_AFTER="$(curl -s -o /dev/null -w '%{http_code}' --max-time 10 \
	"http://localhost:${PORT}/pause.php?ms=0")"
if [ "$K_AFTER" = "200" ]; then
	ok "K: served again once the load subsided"
else
	bad "K: got $K_AFTER after the stall cleared"
fi

K_METRICS="$(docker exec "$SRV" wget -qO- http://127.0.0.1:9090/metrics 2>/dev/null)"
if printf '%s' "$K_METRICS" | grep -q '^oxphp_accept_stalled 0$'; then
	ok "K: the gauge is back to 0 with the stall over"
else
	bad "K: oxphp_accept_stalled did not return to 0"
fi
if printf '%s' "$K_METRICS" | grep -qE '^oxphp_accept_stalls_total [1-9]'; then
	ok "K: the connections that had to wait were counted"
else
	bad "K: oxphp_accept_stalls_total never moved — a stall between two scrapes leaves no trace"
fi

# ── L: the rate limit does not outlive the stall it suppresses ───────
# One report per waiting connection would make the log its own outage, so a
# recent report holds the next one back. Nothing re-enters the loop's waiting
# branch while it is parked, though: a suppression decided on entry and never
# revisited leaves a stall that started inside the window silent for its whole
# life — an hour of an unanswering node under one stale line, which is the
# state this file's K scenario exists to remove.
#
# Two stalls, deliberately close together. The first is brief and reported at
# once. The second starts about a second later, inside the window, and lasts:
# 12 s handlers against a 30 s admission budget mean nothing frees a permit
# while it is being checked. Both halves are asserted — silent while the
# window is open, reported once it closes — because a build that simply
# stopped rate-limiting would pass the second half alone.
docker rm -f "$SRV" >/dev/null 2>&1
docker run -d --name "$SRV" \
	-e DOCUMENT_ROOT=/var/www/html -e PHP_WORKERS=1 \
	-e QUEUE_CAPACITY=4 -e QUEUE_MAX_WAITING=4 -e QUEUE_WAIT_TIMEOUT_MS=30000 \
	-e MAX_CONNECTIONS=8 -e INTERNAL_ADDR=0.0.0.0:9090 -e LOG_LEVEL=info \
	-p "${PORT}":80 -v "$FIX:/var/www/html:ro" \
	"$IMAGE" >/dev/null
L_UP=""
for _ in $(seq 1 30); do
	curl -fsS "http://localhost:${PORT}/pause.php?ms=0" >/dev/null 2>&1 && { L_UP=1; break; }
	sleep 1
done
if [ -n "$L_UP" ]; then
	ok "L: container up (same budget as K, 30 s admission budget)"
else
	bad "L: container failed to start"; docker logs "$SRV" 2>&1 | tail -5 >&2; exit 1
fi

warns() { docker logs "$SRV" 2>&1 | grep -c 'accept loop parked'; }

# First stall: nine short requests take the budget, the ninth connection has
# to wait, and with no report behind it that wait is reported immediately.
for _ in $(seq 1 9); do
	curl -s -o /dev/null --max-time 30 \
		"http://localhost:${PORT}/pause.php?ms=100" >/dev/null 2>&1 &
done
wait
if [ "$(warns)" -eq 1 ]; then
	ok "L: the first stall was reported when it began"
else
	bad "L: expected exactly 1 report from the first stall, got $(warns) — the rest of L proves nothing"
fi

# Second stall, seconds after the first and far longer.
for _ in $(seq 1 9); do
	curl -s -o /dev/null --max-time 60 \
		"http://localhost:${PORT}/pause.php?ms=12000" >/dev/null 2>&1 &
done

L_STALLED=""
for _ in $(seq 1 12); do
	if docker exec "$SRV" wget -qO- http://127.0.0.1:9090/metrics 2>/dev/null \
		| grep -q '^oxphp_accept_stalled 1$'; then L_STALLED=1; break; fi
	sleep 0.25
done
if [ -n "$L_STALLED" ]; then
	ok "L: the second stall has the loop parked again"
else
	bad "L: the loop never parked a second time — the rest of L proves nothing"
fi

# Still inside the window opened by the first report: the second stall is
# under way and deliberately unreported. Without this half, a build that
# dropped rate limiting entirely would look correct.
if [ "$(warns)" -eq 1 ]; then
	ok "L: a stall arriving right after a reported one is held back at first"
else
	bad "L: got $(warns) reports while the window was still open — the rate limit is not holding anything back"
fi

# Past the window, with the stall still going: this is the line a build that
# decides suppression once and never revisits it never writes.
sleep 5
L_AFTER_WARNS="$(warns)"
L_STILL="$(docker exec "$SRV" wget -qO- http://127.0.0.1:9090/metrics 2>/dev/null \
	| awk '/^oxphp_accept_stalled /{print $2}')"
if [ "${L_STILL:-0}" = "1" ]; then
	ok "L: the stall is still going when the window closes"
else
	bad "L: the loop unparked before the window closed (gauge ${L_STILL:-?}) — the check below proves nothing"
fi
if [ "$L_AFTER_WARNS" -ge 2 ]; then
	ok "L: the ongoing stall is reported once the window closes ($L_AFTER_WARNS reports)"
else
	bad "L: still $L_AFTER_WARNS report(s) — a stall that began inside the window stays silent for as long as it lasts"
fi

# The handlers outlive the checks by design; drop the container rather than
# waiting out twelve seconds of sleeps that have nothing left to prove.
docker rm -f "$SRV" >/dev/null 2>&1
wait

# ── M: an application that calls back into this same server ──────────
# selfcall.php holds the only worker while it fetches /pause.php from this same
# instance, and reports what that inner call came back as. The distinction the
# fixture carries — a 529 versus a stream timeout — is the whole scenario: both
# make the outer request slow, and only one of them is the queue answering.
start_selfcall_container() {
	# start_selfcall_container <queue_wait_timeout_ms> [queue_capacity]
	# Capacity 0 is the product's own "auto" — worker_count × 128, the default an
	# ordinary deployment runs with. M2 is about that default, so it asks for it
	# by name rather than by leaving the variable out.
	docker rm -f "$SRV" >/dev/null 2>&1
	docker run -d --name "$SRV" \
		-e DOCUMENT_ROOT=/var/www/html \
		-e PHP_WORKERS=1 \
		-e QUEUE_CAPACITY="${2:-0}" \
		-e QUEUE_WAIT_TIMEOUT_MS="$1" \
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

# selfcall.php reports `inner=<code|timeout> waited=<ms>`; read both out of the
# body rather than timing the outer request, whose own duration also carries
# the delay it was asked for and the pool's scheduling.
inner_of()  { sed -n 's/.*inner=\([^ ]*\).*/\1/p' "$1"; }
waited_of() { sed -n 's/.*waited=\([0-9]*\)ms.*/\1/p' "$1"; }

# ── M1: no free slot — the inner call waits at the gate ──────────────
# QUEUE_CAPACITY=1, with a filler request parked in that one slot before the
# self-caller reaches out: the inner call finds the gate shut and can only
# wait. Nothing can open it, because opening it means the worker taking the
# filler, and the worker is inside the outer request. Measured against the same
# run with the budget off, which is the comparison the behaviour is about.
if start_selfcall_container 1000 1; then
	ok "M1: container up (1 worker, queue capacity 1, budget 1000 ms)"
else
	bad "M1: container failed to start"; docker logs "$SRV" 2>&1 | tail -5 >&2; exit 1
fi

# selfcall first, so it is the one holding the worker; the filler second, so it
# takes the queue slot rather than the worker; and the inner call last, a full
# second after the filler, so which wait is under test is decided here and not
# by whichever request the scheduler happened to run first.
selfcall_with_filler() {
	# selfcall_with_filler <out-file>
	curl -s -o "$1" --max-time 90 \
		"http://localhost:${PORT}/selfcall.php?d=1500&t=20" &
	local outer=$!
	sleep 0.4
	curl -s -o /dev/null --max-time 90 "http://localhost:${PORT}/pause.php?ms=5000" &
	# Read the gate where the inner call meets it, not before. The outer
	# handler holds the worker for 1.5 s before calling back, so a reading
	# taken while it is still sleeping describes a moment nothing in this
	# scenario was measured at — and the filler could still have lost its race
	# by the time the reading was supposed to mean something. Two seconds in,
	# the inner call has been parked for about half its budget and has about
	# half of it left.
	sleep 1.6
	M_SLOTS="$(gauge 'oxphp_admission_slots_available')"
	wait "$outer"
}

selfcall_with_filler "$TMP/m1.budget"
M1_INNER="$(inner_of "$TMP/m1.budget")"
M1_WAITED="$(waited_of "$TMP/m1.budget")"
M1_WASTED="$(gauge 'oxphp_admission_wait_wasted_total')"
M1_REFUSED="$(gauge 'oxphp_admission_refused_total{reason="wait_timeout"}')"

# Without this the scenario cannot claim to be about the gate at all: a run
# where the filler lost its race leaves a free slot, the inner call is admitted
# to the queue, and M1 silently becomes M2.
if [ "${M_SLOTS:--1}" = "0" ]; then
	ok "M1: the gate was shut when the inner call reached it (slots_available 0)"
else
	bad "M1: slots_available was ${M_SLOTS:-?} — the inner call did not meet a full queue, the checks below are about something else"
fi
if [ "$M1_INNER" = "529" ]; then
	ok "M1: the inner call is refused by the gate (529), not left to time out"
else
	bad "M1: inner call came back '$M1_INNER' — the gate never answered it"
fi
if [ "${M1_WAITED:-0}" -ge 800 ]; then
	ok "M1: and it spent the budget doing so (${M1_WAITED} ms of a 1000 ms budget)"
else
	bad "M1: inner call waited only ${M1_WAITED:-?} ms — the gate did not park it"
fi

# The whole point of the series: this wait could not have succeeded, and
# nothing in the response says so. A 529 here is indistinguishable from the
# 529 a genuinely overloaded pool returns, and an operator reading only that
# cannot tell "add workers" from "stop calling yourself".
if [ "${M1_WASTED:--1}" -ge 1 ]; then
	ok "M1: counted as a wait that bought nothing (oxphp_admission_wait_wasted_total ${M1_WASTED})"
else
	bad "M1: oxphp_admission_wait_wasted_total is ${M1_WASTED:-?} — the one wait that provably could not succeed was not recorded as such"
fi

# The series is documented as a subset of the refusals, and this is the only
# place that claim is exercised by a running server: the gate is the other
# site that increments it, and it does so on the line after the refusal. A
# build where the two came apart would still pass every check above.
if [ "${M1_REFUSED:--1}" -ge "${M1_WASTED:-0}" ] && [ "${M1_REFUSED:--1}" -ge 1 ]; then
	ok "M1: and it stayed a subset — ${M1_WASTED} wasted of ${M1_REFUSED} refused on the budget"
else
	bad "M1: oxphp_admission_wait_wasted_total ${M1_WASTED:-?} against reason=\"wait_timeout\" ${M1_REFUSED:-absent} — the wasted count is not a subset of the refusals it claims to narrow"
fi

# Same shape with the budget off: the refusal is the same, its cost is not.
# This difference is what the wait budget trades away on this pattern.
if start_selfcall_container 0 1; then
	ok "M1: control container up (same, QUEUE_WAIT_TIMEOUT_MS=0)"
else
	bad "M1: control container failed to start"
fi
selfcall_with_filler "$TMP/m1.failfast"
M1_FF_INNER="$(inner_of "$TMP/m1.failfast")"
M1_FF_WAITED="$(waited_of "$TMP/m1.failfast")"
if [ "$M1_FF_INNER" = "529" ] && [ "${M1_FF_WAITED:-9999}" -lt 200 ]; then
	ok "M1: fail-fast answers the same 529 in ${M1_FF_WAITED} ms — the budget costs $((M1_WAITED - M1_FF_WAITED)) ms of worker occupancy per self-call"
else
	bad "M1: control gave inner='$M1_FF_INNER' after ${M1_FF_WAITED:-?} ms — the comparison proves nothing"
fi
docker rm -f "$SRV" >/dev/null 2>&1
wait

# ── M2: a slot is free — the inner call waits in the queue ───────────
# The default QUEUE_CAPACITY is worker_count × 128, so on an ordinary
# deployment the inner call never meets the gate at all: it is admitted to the
# queue in front of a pool whose only worker is the outer request. No worker
# will ever pick it up, so a deadline read only at pickup would never be read
# at all — which is what this scenario exists to catch.
if start_selfcall_container 1000; then
	ok "M2: container up (1 worker, default queue capacity, budget 1000 ms)"
else
	bad "M2: container failed to start"; docker logs "$SRV" 2>&1 | tail -5 >&2; exit 1
fi

M2_BEFORE="$(gauge 'oxphp_admission_refused_total{reason="wait_timeout"}')"
# The inner call asks for a handler that records having run. Removed rather
# than created: the file has to be written by the PHP process, which is not the
# user this exec runs as.
docker exec "$SRV" rm -f /tmp/oxphp-inner-ran >/dev/null 2>&1
curl -s -o "$TMP/m2" --max-time 90 "http://localhost:${PORT}/selfcall.php?t=20&i=mark.php"
M2_INNER="$(inner_of "$TMP/m2")"
M2_WAITED="$(waited_of "$TMP/m2")"
# The refused request is still in the queue when the outer one ends, and the
# freed worker reaches it a moment later. Scraping the instant curl returns
# would read the counter before that second event, which is the one the check
# below exists to catch — so let it happen first.
sleep 1
M2_AFTER="$(gauge 'oxphp_admission_refused_total{reason="wait_timeout"}')"

if [ "$M2_INNER" = "529" ]; then
	ok "M2: a queued request nobody picks up is still answered by its deadline (529)"
else
	bad "M2: inner call came back '$M2_INNER' after ${M2_WAITED:-?} ms — QUEUE_WAIT_TIMEOUT_MS bounded nothing"
fi
if [ "${M2_WAITED:-0}" -ge 800 ] && [ "${M2_WAITED:-99999}" -le 3000 ]; then
	ok "M2: and answered on the budget, not whenever (${M2_WAITED} ms of 1000 ms)"
else
	bad "M2: inner call waited ${M2_WAITED:-?} ms against a 1000 ms budget"
fi
# The response alone cannot say who refused it — 529 is also something the
# application could return. The counter is what ties it to admission control.
#
# Exactly one, not merely more than none. The request stays in the queue after
# its deadline is answered, and the worker that eventually reaches it finds an
# expired request too: one refusal reported as two would make the series
# overcount precisely on the pattern it is meant to measure.
if [ "${M2_BEFORE:--1}" -ge 0 ] && [ "${M2_AFTER:--1}" -eq $((M2_BEFORE + 1)) ]; then
	ok "M2: counted once under reason=\"wait_timeout\" ($M2_BEFORE → $M2_AFTER)"
else
	bad "M2: oxphp_admission_refused_total{reason=\"wait_timeout\"} went ${M2_BEFORE:-?} → ${M2_AFTER:-?}, expected exactly one more"
fi

M2_WASTED="$(gauge 'oxphp_admission_wait_wasted_total')"
if [ "${M2_WASTED:--1}" -ge 1 ]; then
	ok "M2: and recorded as a wait that bought nothing (oxphp_admission_wait_wasted_total ${M2_WASTED})"
else
	bad "M2: oxphp_admission_wait_wasted_total is ${M2_WASTED:-?} — a wait nobody could have ended was counted as an ordinary refusal"
fi

# The refused request is not merely unanswered-twice: it must never run. Its
# client was answered a second ago, so a worker that reaches it and executes it
# anyway spends a worker on nobody — and runs whatever the handler does to the
# world for a request the server has already refused. The response of such a
# run goes to a dropped channel and leaves no trace, which is why this asks the
# handler instead.
M2_RAN="$(docker exec "$SRV" sh -c 'cat /tmp/oxphp-inner-ran 2>/dev/null | wc -c' 2>/dev/null | tr -d ' \r')"
if [ "${M2_RAN:-x}" = "0" ]; then
	ok "M2: and never ran — the worker that reached it afterwards dropped it"
else
	bad "M2: the inner handler left ${M2_RAN:-?} mark(s) — a request already answered with 529 was executed anyway"
fi
# Vacuous otherwise: a mark that never appears proves nothing until the same
# path is shown to leave one when the request is actually served.
curl -s -o /dev/null --max-time 30 "http://localhost:${PORT}/mark.php"
M2_CONTROL="$(docker exec "$SRV" sh -c 'cat /tmp/oxphp-inner-ran 2>/dev/null | wc -c' 2>/dev/null | tr -d ' \r')"
if [ "${M2_CONTROL:-x}" = "1" ]; then
	ok "M2: control — the same handler served directly does leave one"
else
	bad "M2: control left ${M2_CONTROL:-?} mark(s) instead of 1 — the check above was testing nothing"
fi

# ── M3: the control the deadline must not catch ──────────────────────
# The budget bounds time spent *not executing*. A request a worker picked up
# inside it runs for as long as it runs — 4 s against a 1 s budget here — and a
# deadline enforced by the clock rather than by whether anyone took the request
# would answer 529 to a perfectly healthy handler three seconds in.
M3_CODE="$(curl -s -o /dev/null -w '%{http_code}' --max-time 60 \
	"http://localhost:${PORT}/pause.php?ms=4000")"
if [ "$M3_CODE" = "200" ]; then
	ok "M3: a request picked up in time runs past the budget and is served (200)"
else
	bad "M3: a 4 s handler under a 1 s budget answered $M3_CODE — the deadline is catching running requests"
fi
docker rm -f "$SRV" >/dev/null 2>&1
wait

# ── N: a wait that failed a race, not a wait that never had one ──────
# A is one half of the wasted-wait claim: the counter does not move when every
# wait ends in a worker. This is the harder half — waits that *fail*, on a pool
# that was picking requests up the whole time they waited. Forty 50 ms requests
# against one worker take about two seconds to get through, so everything past
# the first twenty or so runs out its budget; the pool took a request off the
# queue every 50 ms while they did. A counter that cannot tell that from a pool
# that took nothing would read "waiting is buying nothing" on the commonest
# overload there is, which is exactly the reading it exists to make possible.
if start_container 1000; then
	ok "N: container up (1 s budget, 50 ms handlers)"
else
	bad "N: container failed to start"; docker logs "$SRV" 2>&1 | tail -5 >&2; exit 1
fi

fire 40 50 n
N_SERVED="$(count n 200)"
N_SHED="$(count n 529)"
# Read before the container goes, and after `fire` has waited for every one of
# them: the refusals are what this is about and the last of them lands a second
# after the burst.
N_WASTED="$(gauge 'oxphp_admission_wait_wasted_total')"

if [ "$N_SERVED" -ge 1 ] && [ "$N_SHED" -ge 1 ]; then
	ok "N: a pool working through its queue still sheds what it cannot reach in time (${N_SERVED} × 200, ${N_SHED} × 529)"
else
	bad "N: got ${N_SERVED} × 200 and ${N_SHED} × 529 — this scenario needs both, or the check below is about a pool that was never busy"
fi

if [ "$N_WASTED" = "0" ]; then
	ok "N: oxphp_admission_wait_wasted_total stayed 0 — these waits lost a race for a worker rather than never having one"
else
	bad "N: oxphp_admission_wait_wasted_total reads ${N_WASTED} (-1 = the series is not exported at all) on a pool that picked up a request every 50 ms — it cannot tell a lost race from no race at all"
fi
docker rm -f "$SRV" >/dev/null 2>&1
wait

say ""
say "passed: $PASS, failed: $FAIL"
[ "$FAIL" -eq 0 ]
