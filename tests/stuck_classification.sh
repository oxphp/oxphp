#!/usr/bin/env bash
#
# Integration test for the supervisor's stuck classification
# (`oxphp_worker_stuck_total{kind}`): a request held past the stuck threshold
# is classified by whether its thread burns CPU and whether its VM answers the
# interrupt the supervisor raises once a scan.
#
#   loop   PHP loop with no call inside it           → cpu,    never c_call
#   sleep  blocked in sleep()                        → io only
#   ccall  one internal call burning CPU (bcrypt)    → c_call, never cpu
#   worker the call-free loop from a worker-mode fiber → cpu,  never c_call
#
# And two shapes for what the supervisor's interrupt must leave alone:
#
#   cleanup the call-free loop, unwound by the drain deadline; its shutdown
#           function runs for more than a scan period while the request is
#           still past the threshold and is interrupted once a scan — and must
#           run to its end, not be cancelled a second time
#   parked  the same, for a worker-mode request parked in oxphp_sleep(): the
#           drain sweep resumes it and unwinds it at its suspend point
#   timeout the call-free loop under set_time_limit(61): the engine ends it
#           without the interrupt handler, and its 5 s shutdown function must
#           run to its end under the interrupts that follow
#
# Each shape gets a container of its own with PHP_WORKERS=1, so every stuck
# count on worker 0 is that shape's. The threshold is fixed at 60 seconds, so a
# run takes a little over a minute — which is why this is NOT wired into
# run_all.sh or CI (like tests/graceful_drain.sh). Run it after touching the
# supervisor, the interrupt handler or the progress tick.
#
# Usage: tests/stuck_classification.sh [IMAGE_REF]   (default: oxphp-oxphp:latest)
set -u

IMAGE="${1:-oxphp-oxphp:latest}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FIX="$ROOT/tests/fixtures/stuck"
SHAPES="loop sleep ccall worker cleanup parked timeout"
PASS=0
FAIL=0
TMP="$(mktemp -d)"

ok()  { printf '  \033[32mPASS\033[0m %s\n' "$1"; PASS=$((PASS + 1)); }
bad() { printf '  \033[31mFAIL\033[0m %s\n' "$1"; FAIL=$((FAIL + 1)); }

free_port() {
	python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()'
}

cleanup() {
	for shape in $SHAPES; do
		docker rm -f "stuck_${shape}_$$" >/dev/null 2>&1
	done
	rm -rf "$TMP"
}
trap cleanup EXIT

start_container() {
	# start_container <shape> — echoes "<http port> <metrics port>"
	local name="stuck_$1_$$" http metrics
	http=$(free_port)
	metrics=$(free_port)
	local mode=()
	if [ "$1" = worker ]; then
		mode=(-e WORKER_FILE=/var/www/html/worker.php)
	fi
	# The shortest drain: SIGTERM, a second, then the deadline's unwind and
	# the two seconds the process gives it.
	if [ "$1" = cleanup ]; then
		mode=(-e DRAIN_TIMEOUT_SECONDS=1)
	fi
	if [ "$1" = parked ]; then
		mode=(-e WORKER_FILE=/var/www/html/worker-cleanup.php -e DRAIN_TIMEOUT_SECONDS=1)
	fi
	# The queue wait is raised so a request that arrives before the worker
	# thread is up waits for it rather than being answered 529 — a
	# worker-mode bootstrap can outlast the default budget, and a request
	# that never reaches the worker leaves nothing to classify.
	docker run -d --name "$name" \
		-e DOCUMENT_ROOT=/var/www/html \
		-e PHP_WORKERS=1 \
		-e QUEUE_WAIT_TIMEOUT_MS=120000 \
		-e INTERNAL_ADDR=0.0.0.0:9090 \
		${mode[@]+"${mode[@]}"} \
		-p "${http}:80" -p "${metrics}:9090" \
		-v "$FIX:/var/www/html:ro" \
		"$IMAGE" >/dev/null || return 1
	for _ in $(seq 1 30); do
		curl -fsS "http://localhost:${metrics}/metrics" >/dev/null 2>&1 && {
			echo "$http $metrics"
			return 0
		}
		sleep 1
	done
	return 1
}

stuck_count() {
	# stuck_count <metrics file> <kind>
	awk -v k="$2" '$1 == "oxphp_worker_stuck_total{worker_id=\"0\",kind=\"" k "\"}" { print $2 }' "$1"
}

drain_cleanup() {
	# drain_cleanup <shape> — the request is past the threshold (checked
	# above), so the supervisor interrupts it once a scan, the shutdown
	# function included.
	local name="stuck_$1_$$" logs
	docker kill -s TERM "$name" >/dev/null
	for _ in $(seq 1 30); do
		[ "$(docker inspect -f '{{.State.Running}}' "$name" 2>/dev/null)" = true ] || break
		sleep 1
	done
	logs="$(docker logs "$name" 2>&1)"
	local cancelled
	cancelled=$(printf '%s' "$logs" | grep -c "Request cancelled (shutdown)")
	if [ "$cancelled" -eq 1 ] \
		&& printf '%s' "$logs" | grep -q "stuck-cleanup: shutdown function finished"; then
		ok "$1: shutdown function ran to its end under the supervisor's interrupts"
	else
		bad "$1: shutdown function cut short (cancellations: $cancelled)"
		printf '%s\n' "$logs" | grep -E "cancelled|stuck-cleanup" | tail -5
	fi
}

timeout_cleanup() {
	# timeout_cleanup <metrics file>
	# The timer may fire after this is reached — on a busy host, later than 61
	# seconds of wall time — so wait for the shutdown function to finish or be
	# ended rather than reading the log once.
	local name="stuck_timeout_$$" logs
	for _ in $(seq 1 90); do
		logs="$(docker logs "$name" 2>&1)"
		printf '%s' "$logs" | grep -qE "stuck-timeout: shutdown function finished|Request cancelled" && break
		sleep 1
	done
	if ! printf '%s' "$logs" | grep -q "Maximum execution time"; then
		bad "timeout: max_execution_time never ended the request"
	elif printf '%s' "$logs" | grep -q "stuck-timeout: shutdown function finished" \
		&& ! printf '%s' "$logs" | grep -q "Request cancelled"; then
		# The premise: the shutdown function ran past the threshold, where
		# the supervisor interrupts it once a scan.
		if [ "$(long_running "$1")" -ge 3 ] 2>/dev/null; then
			ok "timeout: shutdown function ran to its end under the supervisor's interrupts"
		else
			bad "timeout: no scan saw the shutdown function past the threshold"
		fi
	else
		bad "timeout: shutdown function cut short"
		printf '%s\n' "$logs" | grep -E "Maximum execution|cancelled|stuck-timeout" | tail -5
	fi
}

long_running() {
	awk '$1 == "oxphp_worker_long_running_total{worker_id=\"0\"}" { print $2 }' "$1"
}

echo "== stuck classification ($IMAGE) =="

for shape in $SHAPES; do
	if ports=$(start_container "$shape"); then
		echo "$ports" > "$TMP/$shape.ports"
	else
		bad "$shape: container failed to start"
		docker logs "stuck_${shape}_$$" 2>&1 | tail -5
		exit 1
	fi
done

# A request that begins before the supervisor's first scan (a second after
# start) is stamped with a start time of 0, which the supervisor reads as an
# idle worker, and it is never classified. Start the requests after that scan.
sleep 2

# Fire every shape at once so the whole run waits out one threshold.
for shape in $SHAPES; do
	read -r http _ < "$TMP/$shape.ports"
	path="/?kind=$shape"
	[ "$shape" = worker ] || [ "$shape" = parked ] && path="/"
	curl -s --max-time 200 "http://localhost:${http}${path}" > /dev/null 2>&1 &
done

# Threshold 60 s, then a few classifying scans a second apart. Wait for them
# rather than for a fixed time: a worker thread that is slow to boot picks its
# request up late, and the request's age counts from then.
deadline=$(( $(date +%s) + 150 ))
for shape in $SHAPES; do
	read -r _ metrics < "$TMP/$shape.ports"
	while [ "$(date +%s)" -lt "$deadline" ]; do
		curl -fsS "http://localhost:${metrics}/metrics" > "$TMP/$shape.metrics" 2>/dev/null
		[ "$(long_running "$TMP/$shape.metrics")" -ge 4 ] 2>/dev/null && break
		sleep 1
	done
done

for shape in $SHAPES; do
	read -r _ metrics < "$TMP/$shape.ports"
	curl -fsS "http://localhost:${metrics}/metrics" > "$TMP/$shape.metrics" 2>/dev/null
	io=$(stuck_count "$TMP/$shape.metrics" io)
	c_call=$(stuck_count "$TMP/$shape.metrics" c_call)
	cpu=$(stuck_count "$TMP/$shape.metrics" cpu)
	seen="io=${io:-?} c_call=${c_call:-?} cpu=${cpu:-?}"
	# A request ended too early is this shape's failure, not a missing
	# premise: read what ended it first.
	if [ "$shape" = timeout ]; then
		timeout_cleanup "$TMP/$shape.metrics"
		continue
	fi
	# The premise: worker 0 held a request past the threshold for the
	# scans this reads. Without it every "never" column holds trivially.
	if [ "$(long_running "$TMP/$shape.metrics")" -lt 3 ] 2>/dev/null \
		|| [ -z "$(long_running "$TMP/$shape.metrics")" ]; then
		bad "$shape: no request held worker 0 past the threshold ($seen)"
		continue
	fi
	case "$shape" in
		loop | worker)
			[ "${cpu:-0}" -ge 3 ] && [ "${c_call:-1}" -eq 0 ] \
				&& ok "$shape: call-free loop reads as cpu ($seen)" \
				|| bad "$shape: call-free loop should read as cpu, never c_call ($seen)" ;;
		sleep)
			[ "${io:-0}" -ge 3 ] && [ "${c_call:-1}" -eq 0 ] && [ "${cpu:-1}" -eq 0 ] \
				&& ok "$shape: sleep reads as io ($seen)" \
				|| bad "$shape: sleep should read as io only ($seen)" ;;
		ccall)
			[ "${c_call:-0}" -ge 3 ] && [ "${cpu:-1}" -eq 0 ] \
				&& ok "$shape: one long internal call reads as c_call ($seen)" \
				|| bad "$shape: one long internal call should read as c_call, never cpu ($seen)" ;;
		cleanup | parked)
			drain_cleanup "$shape" ;;
	esac
done

echo "== ${PASS} passed, ${FAIL} failed =="
[ "$FAIL" -eq 0 ]
