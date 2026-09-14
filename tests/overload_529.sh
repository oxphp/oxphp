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
# Every scenario runs with PHP_WORKERS=1. Most also run with QUEUE_CAPACITY=1,
# so the pool holds exactly one request in a worker and one in the queue and
# anything beyond that has to wait for admission; the ones needing room for
# more say so where they start their container. M2 is the deliberate
# exception: the behaviour it is about only exists where a queue slot is free,
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
#   O: a client that leaves while its request is still queued is counted as a
#      cancellation, and as nothing else — no overload refusal is charged to
#      it and no ERROR line is written for it.
#   P: the same for a client that leaves while its request is running: the
#      fatal this server raises to unwind the handler is not the application
#      failing, and is not logged as one — while a handler that raises a fatal
#      of its own carrying that same wording, on a request nobody cancelled,
#      still is.
#   Q: O and P through the worker-mode receive loop, which takes requests off
#      the queue from a different place and unwinds through the scheduler.
#   R: clients less patient than the budget. Under sustained overload a FIFO
#      queue hands every worker the oldest surviving request, so a fixed budget
#      makes each served request wait it out in full and the client is gone
#      before the handler returns — the pool runs flat out and delivers nothing.
#      The budget has to shorten itself until the work it admits is work
#      somebody is still waiting for, and lengthen again once the queue drains.
#   S: the same load on the per-request pool, which answers a request whose
#      client left while it was queued before starting PHP at all. That costs
#      the pool almost nothing and says the same thing about the wait, so it
#      counts the same — the pool has to come out of the collapse here too.
#   T: the negative control for the shortening itself. Once the budget is
#      below the gap between two pickups, a wait that ends on it ends between
#      them, and the wasted-wait series — which means "the pool began nothing
#      at all while this request waited" — would follow the budget down and
#      report a pool that is serving as one that picks up nothing.
#   U: a client closing a stream it already has is not abandoned work. The
#      flush that finds an SSE connection gone writes the same client-abort
#      reason as a departure mid-handler, and counting it would let every
#      ordinary end of a session shorten the budget, on either pool model.
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
	# start_container <queue_wait_timeout_ms> [queue_max_waiting] [queue_max_waiting_bytes] [queue_capacity] [log_level]
	docker rm -f "$SRV" >/dev/null 2>&1
	docker run -d --name "$SRV" \
		-e DOCUMENT_ROOT=/var/www/html \
		-e PHP_WORKERS=1 \
		-e QUEUE_CAPACITY="${4:-1}" \
		-e QUEUE_WAIT_TIMEOUT_MS="$1" \
		-e QUEUE_MAX_WAITING="${2:-0}" \
		-e QUEUE_MAX_WAITING_BYTES="${3:-0}" \
		-e INTERNAL_ADDR=0.0.0.0:9090 \
		-e LOG_LEVEL="${5:-error}" \
		-p "${PORT}":80 \
		-v "$FIX:/var/www/html:ro" \
		"$IMAGE" >/dev/null || return 1
	for _ in $(seq 1 30); do
		curl -fsS "http://localhost:${PORT}/pause.php?ms=0" >/dev/null 2>&1 && return 0
		sleep 1
	done
	return 1
}

# The same pool, reached through the worker-mode receive loop instead of the
# traditional one. Same knobs; the URI carries no script name because every
# request goes to the entry file.
start_worker_container() {
	# start_worker_container <queue_wait_timeout_ms> [queue_capacity] [log_level]
	docker rm -f "$SRV" >/dev/null 2>&1
	docker run -d --name "$SRV" \
		-e DOCUMENT_ROOT=/var/www/html \
		-e ENTRY_FILE=/var/www/html/worker_entry.php \
		-e WORKER_MODE_ENABLED=true \
		-e PHP_WORKERS=1 \
		-e QUEUE_CAPACITY="${2:-1}" \
		-e QUEUE_WAIT_TIMEOUT_MS="$1" \
		-e INTERNAL_ADDR=0.0.0.0:9090 \
		-e LOG_LEVEL="${3:-error}" \
		-p "${PORT}":80 \
		-v "$FIX:/var/www/html:ro" \
		"$IMAGE" >/dev/null || return 1
	for _ in $(seq 1 30); do
		curl -fsS "http://localhost:${PORT}/?ms=0" >/dev/null 2>&1 && return 0
		sleep 1
	done
	return 1
}

# ERROR-level lines the server has written since it started. A client that
# stops waiting is the commonest thing a public listener sees, and the
# container runs at LOG_LEVEL=error, so on these scenarios this is either zero
# or the whole complaint.
#
# JSON is the only shape this can take: the one subscriber the server installs
# calls .json() unconditionally and no setting changes it, so a bare-word level
# is not a second format to also match but one that never appears.
error_lines() {
	docker logs "$SRV" 2>&1 | grep -c '"level":"ERROR"' | tr -d ' \r'
}

# Log lines reporting the fatal this server raises to unwind a handler whose
# client left, at whatever level they were written. Counted separately from
# error_lines because the fatal reaches the log by two routes with two levels —
# the structured error callback and PHP's own error log through the SAPI hook —
# and a container quiet enough to hide one of them would make a check about it
# pass by filtering rather than by the change under test.
cancel_lines() {
	docker logs "$SRV" 2>&1 | grep -c 'Request cancelled (client_abort)' | tr -d ' \r'
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

# The negative control for scenario R. The budget shortens itself when the work
# the pool starts turns out to belong to clients who have left; here every one
# of the six was delivered, so it must not have moved. Without this, R is
# satisfied by a server that simply always waits less than it was told to.
A_BUDGET="$(gauge 'oxphp_admission_wait_budget_us')"
A_CEILING="$(gauge 'oxphp_admission_wait_budget_ceiling_us')"
if [ "$A_BUDGET" = "1000000" ] && [ "$A_CEILING" = "1000000" ]; then
	ok "A: oxphp_admission_wait_budget_us still reads the configured 1000 ms"
else
	bad "A: oxphp_admission_wait_budget_us reads ${A_BUDGET} against a ceiling of ${A_CEILING} (-1 = the series is not exported at all) — the budget moved on a burst the pool served in full"
fi

# And the controller's only input, on the same burst. Every one of the six was
# delivered to a client that was still there, so the signal the budget moves on
# has to be silent — otherwise the gauge above is standing still for some
# reason other than the absence of evidence.
A_ABANDONED="$(gauge 'oxphp_abandoned_work_total')"
if [ "$A_ABANDONED" = "0" ]; then
	ok "A: oxphp_abandoned_work_total stayed 0 — nothing was served to a client that had gone"
else
	bad "A: oxphp_abandoned_work_total reads ${A_ABANDONED} (-1 = the series is not exported at all) — a burst the pool served in full cannot contain work nobody was waiting for"
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

# Fail-fast has no wait, so it has no budget to publish and no ceiling to
# publish it against. Both series say so with a zero rather than by being
# absent — an absent series and a server that forgot to wire the probe look the
# same to an alert, and a non-zero here would mean a mode with no wait is
# advertising one.
D_BUDGET="$(gauge 'oxphp_admission_wait_budget_us')"
D_CEILING="$(gauge 'oxphp_admission_wait_budget_ceiling_us')"
if [ "$D_BUDGET" = "0" ] && [ "$D_CEILING" = "0" ]; then
	ok "D: the wait-budget pair reads 0 in fail-fast mode"
else
	bad "D: oxphp_admission_wait_budget_us reads ${D_BUDGET} against a ceiling of ${D_CEILING} with QUEUE_WAIT_TIMEOUT_MS=0 (-1 = the series is not exported at all)"
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

# ── O: a client that leaves is counted, and counted as itself ────────
# The whole point of oxphp_request_cancelled_total{reason="client_abort"} is
# to tell a client giving up apart from the server failing, and the commonest
# shape of the first — a client that walks away from a request still waiting
# for a worker — used to move nothing at all. Its only trace was an ERROR line
# saying "Connection error", so an ordinary client timeout read as a fault of
# the server's and the cancellation read as nothing.
#
# What the worker finds when it finally reaches those requests is the other
# half: their budget has run out, and refusing them counted a wait_timeout
# apiece. That series is read as the case for shortening QUEUE_WAIT_TIMEOUT_MS
# — an argument about the pool, which a client's own patience has no business
# making, and which nobody received in any case.
#
# Capacity 4 so all three impatient clients sit in the queue proper rather than
# parking at the gate: it is the queue pickup that used to charge them.
if start_container 1500 0 0 4; then
	ok "O: container up (1.5 s budget, queue capacity 4)"
else
	bad "O: container failed to start"; docker logs "$SRV" 2>&1 | tail -5 >&2; exit 1
fi

# t=0: the blocker takes the only worker for 3 s. t=0.3: three clients queue
# behind it, each giving up at t=1.1 — before their 1.5 s budget runs out at
# t=1.8, and long before the worker is free at t=3. Both margins are load-
# bearing: leave after the budget and the waiting side refuses them as an
# overload, which is the thing under test.
curl -s -o /dev/null --max-time 60 "http://localhost:${PORT}/pause.php?ms=3000" &
sleep 0.3
for i in 1 2 3; do
	curl -s -o /dev/null --max-time 0.8 \
		"http://localhost:${PORT}/pause.php?ms=100" >/dev/null 2>&1 &
done

# Negative control. Without it every count below is satisfied by a run where
# the three never reached the queue — refused at the gate, or never sent — and
# zero would mean nothing.
#
# Read at t=0.7, a clear 0.4 s before the earliest departure — the margin F
# and I leave for the same measurement, and it is the scrape that needs it:
# `pending` goes through `docker exec`, which on Docker Desktop is worth
# hundreds of milliseconds, and one arriving late finds the three already
# gone and fails a build that did everything right.
sleep 0.4
O_PENDING="$(pending)"
if [ "$O_PENDING" -eq 4 ]; then
	ok "O: all three impatient requests really were queued behind the blocker"
else
	bad "O: expected 4 in flight, got ${O_PENDING} — the rest of O proves nothing"
fi

# t≈4.0: the blocker is done, the worker has been through all three, and
# whatever each pickup decided has already been counted.
sleep 3.3
O_ABORTS="$(gauge 'oxphp_request_cancelled_total{reason="client_abort"}')"
O_WAIT_TIMEOUT="$(gauge 'oxphp_admission_refused_total{reason="wait_timeout"}')"
O_WASTED="$(gauge 'oxphp_admission_wait_wasted_total')"
O_ERRORS="$(error_lines)"
wait

if [ "$O_ABORTS" = "3" ]; then
	ok "O: three departed clients counted as three cancellations"
else
	bad "O: oxphp_request_cancelled_total{reason=\"client_abort\"} reads ${O_ABORTS} (-1 = not exported), expected 3 — the commonest client cancellation there is moves the counter by nothing"
fi

if [ "$O_WAIT_TIMEOUT" = "0" ]; then
	ok "O: no overload refusal was charged to a client that had gone"
else
	bad "O: oxphp_admission_refused_total{reason=\"wait_timeout\"} reads ${O_WAIT_TIMEOUT}, expected 0 — refusals nobody received are being counted as refusals the pool handed out"
fi

if [ "$O_WASTED" = "0" ]; then
	ok "O: oxphp_admission_wait_wasted_total stayed 0"
else
	bad "O: oxphp_admission_wait_wasted_total reads ${O_WASTED} — a departed client moved the series that argues for a shorter budget"
fi

if [ "$O_ERRORS" = "0" ]; then
	ok "O: no ERROR line for a client that simply stopped waiting"
else
	bad "O: ${O_ERRORS} ERROR line(s) for clients that stopped waiting — an ordinary client timeout reads as a server fault"
fi
docker rm -f "$SRV" >/dev/null 2>&1
wait

# ── P: an interrupted handler is a cancellation, not an error ────────
# The other end of the same story: a client that leaves while its script is
# actually running. The engine unwinds the handler with a fatal of this
# server's own making, and a fatal is logged at ERROR — so a cancellation the
# server itself initiated was reported as an application failure, twice, from
# the PHP error handler and from the SAPI log hook.
#
# spin.php rather than pause.php: a cancellation is delivered at an opcode
# boundary and usleep() has none, so a request aborted mid-sleep is not
# interrupted at all and this scenario would be about nothing.
#
# LOG_LEVEL=warn, not the error the other scenarios run at: the fatal reaches
# the log twice, once through the structured error callback and once through
# PHP's own error log, and the second of those was a WARN. At LOG_LEVEL=error
# it is filtered out before it is written, and a check counting it would pass
# on a build that never changed it.
if start_container 1000 0 0 1 warn; then
	ok "P: container up"
else
	bad "P: container failed to start"; docker logs "$SRV" 2>&1 | tail -5 >&2; exit 1
fi

curl -s -o /dev/null --max-time 1.0 "http://localhost:${PORT}/spin.php?ms=4000" \
	>/dev/null 2>&1 &
P_PID=$!

# Negative control: the request is in a worker and running, not queued, not
# refused. Read at t=0.5, half a second before its client leaves at t=1.0 —
# the scrape goes through `docker exec`, which is worth hundreds of
# milliseconds here, and one arriving after the departure reads 0 and fails a
# correct build.
sleep 0.5
P_PENDING="$(pending)"
if [ "$P_PENDING" -eq 1 ]; then
	ok "P: the request was running in a worker when its client left"
else
	bad "P: expected 1 in flight, got ${P_PENDING} — the rest of P proves nothing"
fi

wait "$P_PID"; P_RC=$?
if [ "$P_RC" -eq 28 ]; then
	ok "P: the client left mid-handler, unanswered (curl 28)"
else
	bad "P: curl exited $P_RC, not 28 — it was answered rather than abandoned mid-handler"
fi

# The interrupt lands at the loop's next opcode, well inside a second.
sleep 1
P_ABORTS="$(gauge 'oxphp_request_cancelled_total{reason="client_abort"}')"
P_ERRORS="$(error_lines)"
P_CANCEL_LINES="$(cancel_lines)"

if [ "$P_ABORTS" = "1" ]; then
	ok "P: the interrupted request counted as one client abort"
else
	bad "P: oxphp_request_cancelled_total{reason=\"client_abort\"} reads ${P_ABORTS}, expected 1"
fi

if [ "$P_ERRORS" = "0" ]; then
	ok "P: an interrupted handler wrote no ERROR line"
else
	bad "P: ${P_ERRORS} ERROR line(s) from a cancellation this server raised itself — a client hanging up reads as an application fatal"
fi

if [ "$P_CANCEL_LINES" = "0" ]; then
	ok "P: neither route the fatal takes reported it above debug"
else
	bad "P: ${P_CANCEL_LINES} line(s) naming the cancellation at warn or above — one of the two routes the fatal takes to the log still reports it"
fi

# And the worker is still usable: quietening the log must not have been done by
# swallowing something the engine needed.
P_AFTER="$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 \
	"http://localhost:${PORT}/pause.php?ms=10")"
if [ "$P_AFTER" = "200" ]; then
	ok "P: the worker served the next request normally (200)"
else
	bad "P: the next request got $P_AFTER — the interrupted one took the worker with it"
fi

# The other half of the same rule: the text alone must not be enough to
# quieten a fatal. forge_cancel_fatal.php raises E_USER_ERROR carrying the
# exact wording the server uses for a cancelled request, on a request nobody
# cancelled — and it has to be reported as loudly as any other fatal, by both
# routes, or any handler could hide its failures by naming them after ours.
P_ERR_BEFORE="$(error_lines)"
P_CANCEL_BEFORE="$(cancel_lines)"
curl -s -o /dev/null --max-time 30 "http://localhost:${PORT}/forge_cancel_fatal.php" \
	>/dev/null 2>&1
sleep 0.5
P_ERR_FORGED=$(( $(error_lines) - P_ERR_BEFORE ))
P_CANCEL_FORGED=$(( $(cancel_lines) - P_CANCEL_BEFORE ))

if [ "$P_ERR_FORGED" -ge 1 ]; then
	ok "P: a fatal wearing the cancellation's wording, on a request nobody cancelled, is still an ERROR"
else
	bad "P: a script hid its own fatal by naming it after the server's cancellation — no ERROR line appeared"
fi

if [ "$P_CANCEL_FORGED" -ge 2 ]; then
	ok "P: both routes reported the forged fatal at warn or above"
else
	bad "P: only ${P_CANCEL_FORGED} of the two log routes reported the forged fatal above debug"
fi
docker rm -f "$SRV" >/dev/null 2>&1
wait

# ── Q: O and P again, through the worker-mode receive loop ───────────
# Worker mode takes requests off the same channel from a different place and
# unwinds an interrupted handler through the fiber scheduler instead of
# straight out of the worker thread. Both halves of the accounting live on
# those paths, so neither is covered by the traditional runs above.
#
# LOG_LEVEL=warn for the same reason as P: one of the two routes the fatal
# takes to the log was a WARN, and a quieter container would hide it.
if start_worker_container 1500 4 warn; then
	ok "Q: worker-mode container up (1.5 s budget, queue capacity 4)"
else
	bad "Q: worker-mode container failed to start"; docker logs "$SRV" 2>&1 | tail -5 >&2; exit 1
fi

# Timed as in O, and for the same two reasons: the three leave before their
# budget runs out, and the control is read a clear 0.4 s before the first of
# them does.
curl -s -o /dev/null --max-time 60 "http://localhost:${PORT}/?ms=3000" &
sleep 0.3
for i in 1 2 3; do
	curl -s -o /dev/null --max-time 0.8 \
		"http://localhost:${PORT}/?ms=100" >/dev/null 2>&1 &
done

sleep 0.4
Q_PENDING="$(pending)"
if [ "$Q_PENDING" -eq 4 ]; then
	ok "Q: all three impatient requests really were queued behind the blocker"
else
	bad "Q: expected 4 in flight, got ${Q_PENDING} — the rest of Q proves nothing"
fi

sleep 3.3
Q_ABORTS="$(gauge 'oxphp_request_cancelled_total{reason="client_abort"}')"
Q_WAIT_TIMEOUT="$(gauge 'oxphp_admission_refused_total{reason="wait_timeout"}')"
wait

if [ "$Q_ABORTS" = "3" ]; then
	ok "Q: three departed clients counted as three cancellations in worker mode"
else
	bad "Q: oxphp_request_cancelled_total{reason=\"client_abort\"} reads ${Q_ABORTS}, expected 3"
fi

if [ "$Q_WAIT_TIMEOUT" = "0" ]; then
	ok "Q: worker-mode pickup charged no overload refusal to a client that had gone"
else
	bad "Q: oxphp_admission_refused_total{reason=\"wait_timeout\"} reads ${Q_WAIT_TIMEOUT}, expected 0 — the worker-mode pickup still bills departed clients"
fi

# P's half, on the same container: a handler interrupted mid-run.
curl -s -o /dev/null --max-time 0.5 "http://localhost:${PORT}/?spin=1&ms=4000" \
	>/dev/null 2>&1 &
Q_PID=$!
wait "$Q_PID"; Q_RC=$?
sleep 1
Q_ERRORS="$(error_lines)"
Q_CANCEL_LINES="$(cancel_lines)"

if [ "$Q_RC" -eq 28 ]; then
	ok "Q: the client left mid-handler, unanswered (curl 28)"
else
	bad "Q: curl exited $Q_RC, not 28 — it was answered rather than abandoned mid-handler"
fi

if [ "$Q_ERRORS" = "0" ]; then
	ok "Q: worker mode wrote no ERROR line for clients that stopped waiting"
else
	bad "Q: ${Q_ERRORS} ERROR line(s) in worker mode for clients that stopped waiting"
fi

if [ "$Q_CANCEL_LINES" = "0" ]; then
	ok "Q: neither route the fatal takes reported it above debug in worker mode"
else
	bad "Q: ${Q_CANCEL_LINES} line(s) naming the cancellation at warn or above in worker mode"
fi

Q_AFTER="$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 \
	"http://localhost:${PORT}/?ms=10")"
if [ "$Q_AFTER" = "200" ]; then
	ok "Q: the worker served the next request normally (200)"
else
	bad "Q: the next request got $Q_AFTER — the interrupted one took the worker with it"
fi
docker rm -f "$SRV" >/dev/null 2>&1
wait

# ── R: clients less patient than the budget ──────────────────────────
# The pool serves 10 requests a second (one worker, 100 ms handler) and is
# offered roughly 30. A FIFO queue hands a worker the oldest request that still
# has a client, and under a fixed 1000 ms budget that request has waited out the
# client's own 400 ms: the handler runs, the client is already gone, and the
# next 100 ms of the worker goes the same way. Every knob here is the pool's
# except the patience, which is the client's and which the server never learns —
# so the budget has to find its own way under it.
#
# Capacity 64 rather than the 1 the other scenarios use, and that is the point:
# the collapse lives where the queue is *not* full. A full queue refuses on the
# spot and the refusal reaches the client in milliseconds; a queue with room
# admits everything and lets it age instead.
#
# Worker mode here and the per-request pool in S, one scenario per model.
# Neither runs a handler for a request whose client left while it was queued —
# both answer it at the pickup, at memory speed — but the pickup is a different
# piece of code in each, and it is where the evidence the budget moves on is
# counted. What the budget decides is what happens to the requests behind
# those: whether the pool keeps handing workers requests whose clients are
# about to leave, or refuses them early enough that the ones it does start are
# still wanted. Measured on this image at 74 of 240 served in worker mode and
# 79 per-request, with the surplus refused rather than left hanging.
start_collapse_container() {
	docker rm -f "$SRV" >/dev/null 2>&1
	docker run -d --name "$SRV" \
		-e DOCUMENT_ROOT=/var/www/html \
		-e WORKER_MODE_ENABLED=true \
		-e ENTRY_FILE=/var/www/html/wpause.php \
		-e PHP_WORKERS=1 \
		-e QUEUE_CAPACITY=64 \
		-e QUEUE_WAIT_TIMEOUT_MS=1000 \
		-e INTERNAL_ADDR=0.0.0.0:9090 \
		-e LOG_LEVEL=error \
		-p "${PORT}":80 \
		-v "$FIX:/var/www/html:ro" \
		"$IMAGE" >/dev/null || return 1
	for _ in $(seq 1 30); do
		curl -fsS "http://localhost:${PORT}/work?ms=0" >/dev/null 2>&1 && return 0
		sleep 1
	done
	return 1
}

R_WAVES=40
R_PER_WAVE=6
R_PATIENCE=0.4

if start_collapse_container; then
	ok "R: container up (worker mode, 1 worker, 100 ms handler, capacity 64, budget 1000 ms)"
else
	bad "R: container failed to start"; docker logs "$SRV" 2>&1 | tail -5 >&2; exit 1
fi

# Sampled while the load runs, not after it. The budget this scenario is about
# is a live quantity that recovers once the queue drains, so a single reading
# taken when the last curl has returned describes the recovery and not the
# overload. Written under its own prefix: `codes r` globs `$TMP/r.*`, and a
# file of gauge readings in there would be counted as HTTP results.
: > "$TMP/rbudget"
rm -f "$TMP/rstop"
(
	while [ ! -f "$TMP/rstop" ]; do
		gauge 'oxphp_admission_wait_budget_us' >> "$TMP/rbudget"
		sleep 0.5
	done
) &
R_SAMPLER=$!

# Open-loop: the waves keep arriving whether or not the pool is keeping up,
# which is what a queue in front of a saturated pool actually faces. A closed
# loop cannot produce this at all — its clients are the ones waiting, so the
# offered rate falls to the served rate and the queue never builds.
#
# In its own subshell so that the `wait` at the end has only the curls to wait
# for: a bare `wait` waits for every background job of the shell it runs in,
# and the sampler above is one of those — it stops on a file this loop has not
# written yet, so waiting for it here would never return.
(
	for w in $(seq 1 "$R_WAVES"); do
		for c in $(seq 1 "$R_PER_WAVE"); do
			curl -s -o /dev/null -w '%{http_code}\n' --max-time "$R_PATIENCE" \
				"http://localhost:${PORT}/work?ms=100" \
				> "$TMP/r.${w}_${c}" 2>&1 &
		done
		sleep 0.2
	done
	wait
)

touch "$TMP/rstop"
wait "$R_SAMPLER" 2>/dev/null

R_ATTEMPTS=$((R_WAVES * R_PER_WAVE))
R_SERVED="$(count r 200)"
R_SHED="$(count r 529)"
# curl prints 000 when it gives up without a response: the request is still
# somewhere in the server, and nothing will ever be delivered for it. This is
# the population the collapse consists of.
R_HUNG="$(count r 000)"
# Lowest reading the budget reached while the load was on. -1 readings are
# failed scrapes, not a budget of -1, and would win a minimum outright.
R_MIN_BUDGET="$(grep -v '^-1$' "$TMP/rbudget" | sort -n | head -1)"

# The premise, read from the run itself rather than assumed: a pool offered
# more than twice what it can serve must have turned most of it away somehow.
# Without this the checks below would pass on a run that was never overloaded —
# 240 requests a fast enough pool served in full would satisfy "at least 25 were
# served" without exercising anything.
if [ "$((R_SERVED + R_SHED + R_HUNG))" -eq "$R_ATTEMPTS" ] \
	&& [ "$((R_SHED + R_HUNG))" -ge 120 ]; then
	ok "R: the pool was genuinely over-offered (${R_SERVED} × 200, ${R_SHED} × 529, ${R_HUNG} unanswered of ${R_ATTEMPTS})"
else
	bad "R: ${R_SERVED} × 200, ${R_SHED} × 529, ${R_HUNG} unanswered of ${R_ATTEMPTS} — this scenario needs a pool that could not keep up, or the checks below are about nothing"
fi

# The half that says the work is granted. A pool serving 10 requests a second
# has some 80 to give across this run. The bar is the one S uses, and for the
# same reason: both models answer a queue-abandoned request at the pickup, so
# the side with the budget pinned at its ceiling sits where S measures it,
# around 43 of 240, against 74 here once the budget moves. 55 leaves the narrow
# margin on the side that would produce a false pass, which is why this reading
# does not carry the scenario alone — see S for what a red here with the
# refusal count and the gauge both green means.
if [ "$R_SERVED" -ge 55 ]; then
	ok "R: the pool kept serving clients less patient than its budget (${R_SERVED} × 200)"
else
	bad "R: only ${R_SERVED} × 200 out of ${R_ATTEMPTS} — the pool was busy the whole run and delivered almost none of it"
fi

# And the half that says it is bounded. Serving some while leaving the rest to
# hang is not the behaviour being asked for: what cannot be served has to be
# refused to a client who is still there to read the refusal, which is what
# distinguishes a 529 from a connection that goes quiet.
if [ "$R_SHED" -ge 40 ]; then
	ok "R: and refused the surplus to clients still waiting (${R_SHED} × 529)"
else
	bad "R: only ${R_SHED} × 529 against ${R_HUNG} unanswered — the surplus was left hanging rather than turned away"
fi

# The mechanism, not the symptom: the counts above could in principle come from
# a pool that got faster. This is the only reading that says the budget itself
# moved, and it has to be taken from the load, which is why it is sampled.
if [ "${R_MIN_BUDGET:--1}" -ge 0 ] && [ "${R_MIN_BUDGET:-99999999}" -le 500000 ]; then
	ok "R: the wait budget shortened itself under the load (${R_MIN_BUDGET} µs of 1000000 µs)"
else
	bad "R: oxphp_admission_wait_budget_us never went below ${R_MIN_BUDGET:-?} µs (-1 = the series is not exported at all) — the budget stayed as configured while the pool served nobody"
fi

# The way out. A budget that shortens and stays short has traded this failure
# for the one the wait exists to prevent: the next burst inside the pool's
# capacity would be shed instead of absorbed. The queue is empty within a
# second of the last client leaving, and recovery is what must follow from that
# and not from a restart.
R_RECOVERED=""
for _ in $(seq 1 20); do
	if [ "$(gauge 'oxphp_admission_wait_budget_us')" = "1000000" ]; then
		R_RECOVERED=1
		break
	fi
	sleep 0.5
done
if [ -n "$R_RECOVERED" ]; then
	ok "R: and went back to the configured budget once the queue drained"
else
	bad "R: oxphp_admission_wait_budget_us stayed at $(gauge 'oxphp_admission_wait_budget_us') µs on an idle pool — the shortening has no way out"
fi

# The input the whole loop runs on. Everything above reads the budget, which is
# the controller's output; without this the scenario is satisfied by a budget
# that moves for some reason of its own.
#
# The bar is low on purpose, and not because the number is: the better this
# works the smaller it gets, since a shortened budget is precisely what stops
# requests reaching a worker for a client who has gone. On the unfixed build
# the same run produces it in the hundreds. What is being asserted is that the
# evidence exists and is not a stray one-off.
R_ABANDONED="$(gauge 'oxphp_abandoned_work_total')"
if [ "${R_ABANDONED:--1}" -ge 10 ]; then
	ok "R: and counted the work it was reacting to (${R_ABANDONED} × abandoned)"
else
	bad "R: oxphp_abandoned_work_total reads ${R_ABANDONED} (-1 = the series is not exported at all) — the budget moved without the evidence that is supposed to move it"
fi

# And the series this load must NOT move. A wasted wait means the pool began
# nothing at all while somebody waited out the whole budget — a wedged pool.
# This pool is the opposite: it starts a request every 100 ms throughout, and
# its budget bottoms out at 125 ms, so every wait that ends on the deadline spans
# a pickup and the reading is false on its own terms. That is what this check
# says here, and it is all it says: the case where the budget falls under the
# pickup spacing and the reading needs the configured window to hold it down is
# scenario T. A handful from the first second is the most this can legitimately
# be.
R_WASTED="$(gauge 'oxphp_admission_wait_wasted_total')"
if [ "${R_WASTED:--1}" -ge 0 ] && [ "${R_WASTED:-99999}" -le 5 ]; then
	ok "R: and left oxphp_admission_wait_wasted_total alone (${R_WASTED}) — a shedding pool is not a wedged one"
else
	bad "R: oxphp_admission_wait_wasted_total reads ${R_WASTED} against ${R_SHED} refusals — the wasted-wait reading is following the budget down and calling a working pool wedged"
fi
docker rm -f "$SRV" >/dev/null 2>&1
wait

# ── S: the same collapse, on the per-request pool ───────────
#
# R covers worker mode. This pool model reaches the same request through a
# worker loop of its own, and answers it 499 there without starting PHP, so it
# costs almost nothing — and it is tempting to conclude that a request that
# cost nothing is not evidence of anything and should not move the budget.
# Measured, that reading costs this pool half of what it can deliver: the cheap
# 499s are most of what it picks up under this load, and leaving them out keeps
# the budget at its ceiling, above the patience in front of it, so the pool
# keeps admitting requests whose clients are gone before a worker frees up. The
# clients that are still there get no answer at all rather than a 529 they
# could act on.
#
# What the counter measures is therefore the wait, not the work: a client that
# left while queued says the waiting outlived it, whatever that request went on
# to cost. This scenario asserts the consequence on the second of the two pool
# models, whose pickup is the second of the two places the evidence is counted
# — it serves, it refuses the surplus to clients still there, and the budget
# comes down and goes back up.
start_percall_container() {
	docker rm -f "$SRV" >/dev/null 2>&1
	docker run -d --name "$SRV" \
		-e DOCUMENT_ROOT=/var/www/html \
		-e PHP_WORKERS=1 \
		-e QUEUE_CAPACITY=64 \
		-e QUEUE_WAIT_TIMEOUT_MS=1000 \
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

if start_percall_container; then
	ok "S: container up (per-request, 1 worker, 100 ms handler, capacity 64, budget 1000 ms)"
else
	bad "S: container failed to start"; docker logs "$SRV" 2>&1 | tail -5 >&2; exit 1
fi

: > "$TMP/sbudget"
rm -f "$TMP/sstop"
(
	while [ ! -f "$TMP/sstop" ]; do
		gauge 'oxphp_admission_wait_budget_us' >> "$TMP/sbudget"
		sleep 0.5
	done
) &
S_SAMPLER=$!

# Same shape as R — see there for why the load loop has a subshell of its own.
(
	for w in $(seq 1 "$R_WAVES"); do
		for c in $(seq 1 "$R_PER_WAVE"); do
			curl -s -o /dev/null -w '%{http_code}\n' --max-time "$R_PATIENCE" \
				"http://localhost:${PORT}/pause.php?ms=100" \
				> "$TMP/s.${w}_${c}" 2>&1 &
		done
		sleep 0.2
	done
	wait
)

touch "$TMP/sstop"
wait "$S_SAMPLER" 2>/dev/null

S_SERVED="$(count s 200)"
S_SHED="$(count s 529)"
S_HUNG="$(count s 000)"
# Lowest reading the budget reached while the load was on — see R.
S_MIN_BUDGET="$(grep -v '^-1$' "$TMP/sbudget" | sort -n | head -1)"

# The premise, as in R: this says nothing unless the pool was over-offered.
if [ "$((S_SERVED + S_SHED + S_HUNG))" -eq "$R_ATTEMPTS" ] \
	&& [ "$((S_SHED + S_HUNG))" -ge 120 ]; then
	ok "S: the pool was genuinely over-offered (${S_SERVED} × 200, ${S_SHED} × 529, ${S_HUNG} unanswered of ${R_ATTEMPTS})"
else
	bad "S: ${S_SERVED} × 200, ${S_SHED} × 529, ${S_HUNG} unanswered of ${R_ATTEMPTS} — this scenario needs a pool that could not keep up"
fi

# The work granted. This pool never collapsed as far as worker mode does — it
# clears the departed from its queue without running them — so the two sides
# are close together and the bar has to be placed with both of them in view:
# around 43 of 240 with the budget pinned at its ceiling, around 75 once it
# moves, against a ceiling of some 80 that one worker on a 100 ms sleep can
# serve in the window. 55 therefore sits 12 above the unfixed build and 20
# below the fixed one, and the narrow side is the one that would produce a
# false pass — which is why this reading does not carry the scenario alone:
# the refusal count below separates the two by two orders of magnitude, and
# the gauge after it reads the mechanism directly. It is also the first of the
# three to suffer on a loaded host: the waves are paced by wall-clock sleeps,
# so the window does not stretch to compensate, and enough contention on the
# docker daemon to cost a quarter of the service rate would bring the fixed
# build down towards this bar. A red here with the refusal count and the gauge
# both green is that, not a regression.
if [ "$S_SERVED" -ge 55 ]; then
	ok "S: the pool kept serving clients less patient than its budget (${S_SERVED} × 200)"
else
	bad "S: only ${S_SERVED} × 200 out of ${R_ATTEMPTS} — the per-request pool is admitting requests whose clients are already gone"
fi

# And bounded. With the budget at its ceiling the refusal arrives after the
# client has given up and is recorded as no answer at all, so this count is
# near zero there and in the hundreds once the budget is under the patience in
# front of it — the cleanest of the three readings.
if [ "$S_SHED" -ge 60 ]; then
	ok "S: and refused the surplus to clients still waiting (${S_SHED} × 529)"
else
	bad "S: only ${S_SHED} × 529 against ${S_HUNG} unanswered — the surplus was left hanging rather than turned away"
fi

# The mechanism behind both.
if [ "${S_MIN_BUDGET:--1}" -ge 0 ] && [ "${S_MIN_BUDGET:-99999999}" -le 250000 ]; then
	ok "S: the wait budget shortened itself here too (${S_MIN_BUDGET} µs of 1000000 µs)"
else
	bad "S: oxphp_admission_wait_budget_us never went below ${S_MIN_BUDGET:-?} µs (-1 = the series is not exported at all) — a request answered 499 before it ran is still a wait that outlived its client"
fi

# And the way out, as in R.
S_RECOVERED=""
for _ in $(seq 1 20); do
	if [ "$(gauge 'oxphp_admission_wait_budget_us')" = "1000000" ]; then
		S_RECOVERED=1
		break
	fi
	sleep 0.5
done
if [ -n "$S_RECOVERED" ]; then
	ok "S: and went back to the configured budget once the queue drained"
else
	bad "S: oxphp_admission_wait_budget_us stayed at $(gauge 'oxphp_admission_wait_budget_us') µs on an idle pool — the shortening has no way out"
fi

# As in R, and load-bearing here in a way it is not there: on this pool model
# the abandoned request is the cheap 499, and counting it is the whole reason
# the budget moves at all. A build that stopped counting it would leave every
# other reading in this scenario looking like the unfixed one.
S_ABANDONED="$(gauge 'oxphp_abandoned_work_total')"
if [ "${S_ABANDONED:--1}" -ge 10 ]; then
	ok "S: and counted the work it was reacting to (${S_ABANDONED} × abandoned)"
else
	bad "S: oxphp_abandoned_work_total reads ${S_ABANDONED} (-1 = the series is not exported at all) — the per-request pool is not reporting the queued-499 as a wait that outlived its client"
fi

# The same negative control as in R.
S_WASTED="$(gauge 'oxphp_admission_wait_wasted_total')"
if [ "${S_WASTED:--1}" -ge 0 ] && [ "${S_WASTED:-99999}" -le 5 ]; then
	ok "S: and left oxphp_admission_wait_wasted_total alone (${S_WASTED})"
else
	bad "S: oxphp_admission_wait_wasted_total reads ${S_WASTED} against ${S_SHED} refusals — the wasted-wait reading is following the budget down and calling a working pool wedged"
fi
docker rm -f "$SRV" >/dev/null 2>&1
wait

# ── T: the wasted-wait reading under a budget the server has shortened ───────
#
# R and S both leave oxphp_admission_wait_wasted_total at zero, and neither of
# them says why. Their budgets bottom out at 125 ms and 250 ms against a
# handler of 100 ms, so a wait that ends on the deadline still spans a pickup,
# and the reading "the pool began nothing while this one waited" is false on
# its own terms. That stops being true as soon as the budget falls below the
# spacing between two starts, and the budget is driven by the patience in front
# of the pool, which nothing here controls.
#
# So: the same pool with a 200 ms handler and clients giving up at 350 ms. The
# budget comes down to 62 ms — under a third of the gap between two pickups — and
# from then on most waits do end between them. The pool is serving the whole
# time, tens of clients get answers, and a reading that followed the budget
# down would report it as one that picks nothing up at all. Measured before the
# window was pinned to the configured budget: 42 served, 227 refused, and 108
# wasted waits on a pool that had started work every 200 ms throughout.
T_WAVES=60
T_PER_WAVE=6
T_PATIENCE=0.35

if start_collapse_container; then
	ok "T: container up (worker mode, 1 worker, 200 ms handler, capacity 64, budget 1000 ms)"
else
	bad "T: container failed to start"; docker logs "$SRV" 2>&1 | tail -5 >&2; exit 1
fi

: > "$TMP/tbudget"
rm -f "$TMP/tstop"
(
	while [ ! -f "$TMP/tstop" ]; do
		gauge 'oxphp_admission_wait_budget_us' >> "$TMP/tbudget"
		sleep 0.5
	done
) &
T_SAMPLER=$!

# Same shape as R — see there for why the load loop has a subshell of its own.
(
	for w in $(seq 1 "$T_WAVES"); do
		for c in $(seq 1 "$T_PER_WAVE"); do
			curl -s -o /dev/null -w '%{http_code}\n' --max-time "$T_PATIENCE" \
				"http://localhost:${PORT}/work?ms=200" \
				> "$TMP/t.${w}_${c}" 2>&1 &
		done
		sleep 0.2
	done
	wait
)

touch "$TMP/tstop"
wait "$T_SAMPLER" 2>/dev/null

T_ATTEMPTS=$((T_WAVES * T_PER_WAVE))
T_SERVED="$(count t 200)"
T_SHED="$(count t 529)"
T_HUNG="$(count t 000)"
T_MIN_BUDGET="$(grep -v '^-1$' "$TMP/tbudget" | sort -n | head -1)"

# Premise one: over-offered, as in R.
if [ "$((T_SERVED + T_SHED + T_HUNG))" -eq "$T_ATTEMPTS" ] \
	&& [ "$((T_SHED + T_HUNG))" -ge 120 ]; then
	ok "T: the pool was genuinely over-offered (${T_SERVED} × 200, ${T_SHED} × 529, ${T_HUNG} unanswered of ${T_ATTEMPTS})"
else
	bad "T: ${T_SERVED} × 200, ${T_SHED} × 529, ${T_HUNG} unanswered of ${T_ATTEMPTS} — this scenario needs a pool that could not keep up"
fi

# Premise two, and the one the whole scenario rests on: the budget went under
# the 200 ms that separates two pickups. Above it every wait spans a start and
# the reading below is held down by that alone, which is exactly what makes R
# and S unable to say anything about this.
if [ "${T_MIN_BUDGET:--1}" -ge 0 ] && [ "${T_MIN_BUDGET:-99999999}" -le 125000 ]; then
	ok "T: the budget came down below the pool's own pickup spacing (${T_MIN_BUDGET} µs against a 200000 µs handler)"
else
	bad "T: oxphp_admission_wait_budget_us never went below ${T_MIN_BUDGET:-?} µs (-1 = the series is not exported at all) — without a budget under the handler the check below is satisfied by a pool nobody shortened anything for"
fi

# Premise three: it was serving. A pool that delivered nothing would be a pool
# this counter is entitled to describe.
if [ "$T_SERVED" -ge 15 ]; then
	ok "T: and kept delivering while it was down there (${T_SERVED} × 200)"
else
	bad "T: only ${T_SERVED} × 200 of ${T_ATTEMPTS} — a pool this idle is not a counter-example to anything"
fi

# The check. Both places a budget can run out are covered: the gate, where a
# request never got a slot, and the queue, where it got one and no worker
# reached it. The second is the one this load produces in bulk — capacity 64
# admits nearly everything and lets it age — and a build that pins the window
# only at the gate reads exactly like one that pins it nowhere.
T_WASTED="$(gauge 'oxphp_admission_wait_wasted_total')"
if [ "${T_WASTED:--1}" -ge 0 ] && [ "${T_WASTED:-99999}" -le 5 ]; then
	ok "T: and left oxphp_admission_wait_wasted_total alone (${T_WASTED}) — a shortened wait is not evidence about the pool"
else
	bad "T: oxphp_admission_wait_wasted_total reads ${T_WASTED} against ${T_SHED} refusals on a pool serving ${T_SERVED} clients — the wasted-wait reading is following the budget down and calling a working pool wedged"
fi
docker rm -f "$SRV" >/dev/null 2>&1
wait

# ── U: a stream its client closes is not abandoned work ──────────────
# oxphp_abandoned_work_total is what the budget shortens on, and it means a
# client that stopped waiting for its response. A stream hands its response
# over with the first flush and goes on running; a client that closes it after
# that has had its answer, and every SSE session ends that way. From inside the
# request the two look alike — the flush that finds the connection gone writes
# the same client-abort reason a client leaving mid-handler does — so they are
# told apart by whether the response had gone out, and this is where that is
# pinned, once per pool model.
#
# Idle pools with nothing queued, so the budget itself cannot move here and the
# counter is what is asserted — against a control in the same container that
# it still counts a client leaving before its response went out.
U_CLIENTS=4

# $1 label, $2 path. Each client takes the first events and leaves at 300 ms,
# a few flushes into a 10 s stream. $3 is the same handler run as an ordinary
# 600 ms request, for the control.
u_streams() {
	local label="$1" path="$2" plain="$3" i got seen counted
	docker exec "$SRV" rm -f /tmp/streams
	for i in $(seq 1 "$U_CLIENTS"); do
		curl -sN --max-time 0.3 "http://localhost:${PORT}${path}" > "$TMP/ubody.$i" 2>/dev/null
	done
	# The next flush after a close notices it within one 50 ms event; a
	# second is room for the four of them to unwind and record it.
	sleep 1

	# Premise one: the streams started, so each response had gone out before
	# its client left. A client that got nothing proves nothing about a
	# response it never had.
	got="$(grep -l '^data: 0$' "$TMP"/ubody.* 2>/dev/null | wc -l | tr -d ' ')"
	if [ "$got" -eq "$U_CLIENTS" ]; then
		ok "U ($label): every client received the stream's first event before closing it (${got}/${U_CLIENTS})"
	else
		bad "U ($label): only ${got}/${U_CLIENTS} clients received a first event — the streams never started, and the check below would pass on a response nobody had"
	fi

	# Premise two: each handler saw its client go, which is what writes the
	# client-abort reason the counter reads. Without it the counter stays
	# still on every build, fixed or not.
	seen="$(docker exec "$SRV" sh -c 'grep -c "^aborted$" /tmp/streams 2>/dev/null || echo 0')"
	if [ "${seen:-0}" -eq "$U_CLIENTS" ]; then
		ok "U ($label): and every stream observed its client leaving (${seen}/${U_CLIENTS} connection_aborted())"
	else
		bad "U ($label): ${seen:-0}/${U_CLIENTS} streams observed their client leave — the stream did not run into the closed connection, so nothing here was put to the counter"
	fi

	counted="$(gauge 'oxphp_abandoned_work_total')"
	if [ "${counted:--1}" -eq 0 ]; then
		ok "U ($label): and oxphp_abandoned_work_total stayed at 0"
	else
		bad "U ($label): oxphp_abandoned_work_total reads ${counted} (-1 = not exported) after ${U_CLIENTS} streams closed by clients that already had them — an ordinary end of an SSE session is being read as a client giving up on the wait"
	fi
	rm -f "$TMP"/ubody.*

	# The control. A build that stopped counting after handlers altogether
	# passes everything above, so the same counter, in the same container,
	# must still see a client that left 200 ms into a 600 ms handler that had
	# sent nothing yet — the departure the budget is tuned against.
	# Read as a step, so the reading above does not decide this one too.
	local before after
	before="$(gauge 'oxphp_abandoned_work_total')"
	curl -s --max-time 0.2 "http://localhost:${PORT}${plain}" >/dev/null 2>&1
	sleep 1
	after="$(gauge 'oxphp_abandoned_work_total')"
	if [ "${before:--1}" -ge 0 ] && [ "$((after - before))" -eq 1 ]; then
		ok "U ($label): while a client leaving an ordinary handler before its answer is still counted (${before} → ${after})"
	else
		bad "U ($label): oxphp_abandoned_work_total went ${before} → ${after} for one client that left a handler which had sent nothing — expected a step of exactly 1; the streams above prove nothing if the counter no longer counts this"
	fi
}

u_start() { # extra docker-run arguments
	docker rm -f "$SRV" >/dev/null 2>&1
	docker run -d --name "$SRV" \
		-e DOCUMENT_ROOT=/var/www/html \
		-e INTERNAL_ADDR=0.0.0.0:9090 \
		-e LOG_LEVEL=error \
		"$@" \
		-p "${PORT}":80 \
		-v "$FIX:/var/www/html:ro" \
		"$IMAGE" >/dev/null || return 1
}

u_ready() { # $1 probe path
	for _ in $(seq 1 30); do
		curl -fsS --max-time 2 "http://localhost:${PORT}$1" >/dev/null 2>&1 && return 0
		sleep 1
	done
	return 1
}

if u_start && u_ready "/stream.php?n=0"; then
	ok "U: container up (per-request pool, SSE)"
	u_streams "per-request" "/stream.php" "/stream.php?n=0&ms=600"
else
	bad "U: per-request container failed to start"; docker logs "$SRV" 2>&1 | tail -5 >&2
fi

if u_start -e WORKER_MODE_ENABLED=true -e ENTRY_FILE=/var/www/html/wstream.php && u_ready "/?n=0"; then
	ok "U: container up (worker mode, SSE)"
	u_streams "worker" "/" "/?n=0&ms=600"
else
	bad "U: worker-mode container failed to start"; docker logs "$SRV" 2>&1 | tail -5 >&2
fi
docker rm -f "$SRV" >/dev/null 2>&1

say ""
say "passed: $PASS, failed: $FAIL"
[ "$FAIL" -eq 0 ]
