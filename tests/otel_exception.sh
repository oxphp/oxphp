#!/usr/bin/env bash
#
# Integration test for full exception data on OTel span events.
#
# Exercises the real FFI capture path that the Rust unit tests (which feed
# mock strings into push_exception_event) cannot reach: the C-side
# oxphp_exception_capture — save/clear/restore EG(exception) around
# getTraceAsString() from the observer-end, the message property read, the
# instanceof-Throwable gate, and the lifetime of the borrowed message pointer.
#
# Scenario AUTO: a #[OxPHP\Apm\Trace] function throws → the exported span
# carries an "exception" event with exception.type, exception.message and
# exception.stacktrace.
#
# Scenario MANUAL: oxphp_apm_error($e) on an explicit span → the same three
# attributes on that span's exception event.
#
# Root-span auto-capture scenarios: UNCAUGHT (raw uncaught exception), FATAL
# (classless E_USER_ERROR), CHAINED (outer wraps a cause — must bucket on the
# thrown class, not the root cause), FORGE (a message forging a "\n\nNext
# FakeClass: …" segment — the structural throw-hook class must win over the text
# parse), STREAM (a 5xx that starts streaming then throws a late fatal — the
# event must still reach the span via the deferred RequestComplete), HANDLED
# (set_exception_handler swallows — no event, with a positive control), WORKER
# (fiber-catch C capture — a handler-body throw must return 500 AND carry class /
# trace / file / line), WORKER-STREAM (a worker handler that commits a 5xx,
# streams, then throws — status 500 on the wire and the late fatal still reaches
# the span), and WORKER-B (a request parked in a suspending shutdown function
# keeps its capture while another request runs on the same worker thread — the
# per-fiber save/restore guard).
#
# Assertion is against an OpenTelemetry collector's debug exporter (stdout).
#
# NOT wired into run_all.sh or CI (like tests/graceful_drain.sh and
# tests/cli_run.sh) — run manually after touching the exception-capture path
# (ext/bridge/oxphp_bridge.c, ext/oxphp_fiber.c, ext/oxphp_sapi.c,
# src/php/unhandled_exception.rs, src/plugins/ox_apm, src/plugins/ox_otel).
#
# Usage: tests/otel_exception.sh [IMAGE_REF]   (default: oxphp-oxphp:latest)
set -u

IMAGE="${1:-oxphp-oxphp:latest}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FIX="$ROOT/tests/fixtures/otel_exception"
NET="oxexc-net"
COL="oxexc-col"
SRV="oxexc-srv"
WRK="oxexc-wrk"
PASS=0
FAIL=0

ok()  { printf '  \033[32mPASS\033[0m %s\n' "$1"; PASS=$((PASS + 1)); }
bad() { printf '  \033[31mFAIL\033[0m %s\n' "$1"; FAIL=$((FAIL + 1)); }

cleanup() {
	docker rm -f "$COL" "$SRV" "$WRK" >/dev/null 2>&1
	docker network rm "$NET" >/dev/null 2>&1
}
trap cleanup EXIT

docker rm -f "$COL" "$SRV" "$WRK" >/dev/null 2>&1
docker network rm "$NET" >/dev/null 2>&1
docker network create "$NET" >/dev/null

docker run -d --name "$COL" --network "$NET" \
	-v "$FIX/otelcol.yaml":/etc/otelcol/config.yaml:ro \
	otel/opentelemetry-collector:latest --config /etc/otelcol/config.yaml >/dev/null

docker run -d --name "$SRV" --network "$NET" \
	-v "$FIX/auto.php":/var/www/html/public/auto.php:ro \
	-v "$FIX/manual.php":/var/www/html/public/manual.php:ro \
	-v "$FIX/latin1.php":/var/www/html/public/latin1.php:ro \
	-v "$FIX/reason.php":/var/www/html/public/reason.php:ro \
	-v "$FIX/anon.php":/var/www/html/public/anon.php:ro \
	-v "$FIX/ref.php":/var/www/html/public/ref.php:ro \
	-v "$FIX/uncaught.php":/var/www/html/public/uncaught.php:ro \
	-v "$FIX/fatal.php":/var/www/html/public/fatal.php:ro \
	-v "$FIX/chained.php":/var/www/html/public/chained.php:ro \
	-v "$FIX/forge.php":/var/www/html/public/forge.php:ro \
	-v "$FIX/stream_fatal.php":/var/www/html/public/stream_fatal.php:ro \
	-v "$FIX/handled.php":/var/www/html/public/handled.php:ro \
	-e OTEL_ENABLED=true -e OTEL_APM_ENABLED=true \
	-e OTEL_EXPORTER_OTLP_ENDPOINT=http://"$COL":4317 \
	-e INTERNAL_ADDR=0.0.0.0:9090 \
	-e LOG_LEVEL=error \
	"$IMAGE" >/dev/null

# Worker-mode server (same collector) — exercises the fiber-catch capture path.
# PHP_WORKERS=1 pins all requests to one worker thread so the scenario-B overlap
# (/a-fail parked in a suspending shutdown function while /b-ok runs) shares the
# single thread's capture slot — the case per-fiber save/restore must protect.
docker run -d --name "$WRK" --network "$NET" \
	-v "$FIX/worker.php":/var/www/html/worker.php:ro \
	-e OTEL_ENABLED=true -e OTEL_APM_ENABLED=true \
	-e OTEL_EXPORTER_OTLP_ENDPOINT=http://"$COL":4317 \
	-e WORKER_MODE_ENABLED=true -e ENTRY_FILE=/var/www/html/worker.php \
	-e PHP_WORKERS=1 \
	-e INTERNAL_ADDR=0.0.0.0:9090 \
	-e LOG_LEVEL=error \
	"$IMAGE" >/dev/null

# Wait for the server's internal health endpoint (max ~30s).
ready=0
for _ in $(seq 1 30); do
	if docker exec "$SRV" wget -q --spider http://127.0.0.1:9090/health 2>/dev/null; then
		ready=1
		break
	fi
	sleep 1
done
[ "$ready" = 1 ] || { echo "server did not become healthy"; docker logs "$SRV" | tail -20; exit 1; }

# Wait for the worker-mode server too.
wready=0
for _ in $(seq 1 30); do
	if docker exec "$WRK" wget -q --spider http://127.0.0.1:9090/health 2>/dev/null; then
		wready=1
		break
	fi
	sleep 1
done
[ "$wready" = 1 ] || { echo "worker server did not become healthy"; docker logs "$WRK" | tail -20; exit 1; }

# Drive both endpoints.
docker run --rm --network "$NET" curlimages/curl:latest \
	-s -o /dev/null -w "auto  HTTP %{http_code}\n" "http://$SRV:80/auto.php"
docker run --rm --network "$NET" curlimages/curl:latest \
	-s -o /dev/null -w "manual HTTP %{http_code}\n" "http://$SRV:80/manual.php"
docker run --rm --network "$NET" curlimages/curl:latest \
	-s -o /dev/null -w "latin1 HTTP %{http_code}\n" "http://$SRV:80/latin1.php"
docker run --rm --network "$NET" curlimages/curl:latest \
	-s -o /dev/null -w "reason HTTP %{http_code}\n" "http://$SRV:80/reason.php"
docker run --rm --network "$NET" curlimages/curl:latest \
	-s -o /dev/null -w "anon  HTTP %{http_code}\n" "http://$SRV:80/anon.php"
docker run --rm --network "$NET" curlimages/curl:latest \
	-s -o /dev/null -w "ref   HTTP %{http_code}\n" "http://$SRV:80/ref.php"
docker run --rm --network "$NET" curlimages/curl:latest \
	-s -o /dev/null -w "uncaught HTTP %{http_code}\n" "http://$SRV:80/uncaught.php"
docker run --rm --network "$NET" curlimages/curl:latest \
	-s -o /dev/null -w "fatal HTTP %{http_code}\n" "http://$SRV:80/fatal.php"
docker run --rm --network "$NET" curlimages/curl:latest \
	-s -o /dev/null -w "chained HTTP %{http_code}\n" "http://$SRV:80/chained.php"
docker run --rm --network "$NET" curlimages/curl:latest \
	-s -o /dev/null -w "forge HTTP %{http_code}\n" "http://$SRV:80/forge.php"
docker run --rm --network "$NET" curlimages/curl:latest \
	-s -o /dev/null -w "stream_fatal HTTP %{http_code}\n" "http://$SRV:80/stream_fatal.php"
# Capture handled's body as a positive control — proves handled.php actually ran
# (its set_exception_handler fired) so the "no event" assertion is meaningful and
# not a false pass from a 404 / parse error.
HANDLED_BODY="$(docker run --rm --network "$NET" curlimages/curl:latest \
	-s "http://$SRV:80/handled.php")"
# Capture the worker status codes: a handler-body throw and a streamed-then-thrown
# fatal must both surface as 500. Asserting the status (not just grepping the
# message) is what catches a silently-200 worker regression that the root-span
# gate would then drop.
WORKER_BOOM_CODE="$(docker run --rm --network "$NET" curlimages/curl:latest \
	-s -o /dev/null -w "%{http_code}" "http://$WRK:80/boom")"
echo "worker /boom HTTP $WORKER_BOOM_CODE"
WORKER_STREAM_CODE="$(docker run --rm --network "$NET" curlimages/curl:latest \
	-s -o /dev/null --max-time 30 -w "%{http_code}" "http://$WRK:80/stream-boom")"
echo "worker /stream-boom HTTP $WORKER_STREAM_CODE"

# Reset the worker's consecutive-error breaker before the scenario-B pair. An
# uncaught handler exception legitimately increments it, and /boom + /stream-boom
# are two consecutive 500s with /a-fail a third — WORKER_MAX_CONSECUTIVE_ERRORS
# (3) would trip and hand /b-ok a "500 PHP Worker Error" from the restarting
# worker. One successful request resets the counter (a test-ordering guard, not a
# behaviour change).
docker run --rm --network "$NET" curlimages/curl:latest \
	-s -o /dev/null -w "worker /ok HTTP %{http_code}\n" "http://$WRK:80/ok"

# Scenario B: /a-fail throws, then parks its capture in a suspending shutdown
# function (signalled by a marker file). /b-ok deterministically spin-waits for
# that marker before returning, so it provably overlaps /a-fail's parked phase on
# the single worker thread — the parked capture must survive that overlap. No
# fixed sleep: /b-ok's cooperative wait removes the timing race entirely. The
# --max-time bounds the run if the marker never appears (a real regression).
docker run --rm --network "$NET" curlimages/curl:latest \
	-s -o /dev/null --max-time 30 "http://$WRK:80/a-fail" &
BOK_BODY="$(docker run --rm --network "$NET" curlimages/curl:latest \
	-s --max-time 30 "http://$WRK:80/b-ok")"
echo "a-fail(bg)/b-ok body: $BOK_BODY"
wait

# Let the batch span processor flush to the collector.
sleep 8

LOGS="$(docker logs "$COL" 2>&1)"

# Scenario AUTO
echo "$LOGS" | grep -qF 'exception.type: Str(RuntimeException)' \
	&& ok "auto: exception.type"   || bad "auto: exception.type"
echo "$LOGS" | grep -qF 'exception.message: Str(auto path: card declined)' \
	&& ok "auto: exception.message" || bad "auto: exception.message"
echo "$LOGS" | grep -qF 'exception.stacktrace: Str(#0' \
	&& ok "auto: exception.stacktrace" || bad "auto: exception.stacktrace"
echo "$LOGS" | grep -qF 'chargeCard()' \
	&& ok "auto: stacktrace has throwing frame" || bad "auto: stacktrace has throwing frame"

# Scenario MANUAL
echo "$LOGS" | grep -qF 'exception.type: Str(LogicException)' \
	&& ok "manual: exception.type"   || bad "manual: exception.type"
echo "$LOGS" | grep -qF 'exception.message: Str(manual path: bad state)' \
	&& ok "manual: exception.message" || bad "manual: exception.message"
echo "$LOGS" | grep -qE 'Name +: manual_span' \
	&& ok "manual: recorded on explicit span" || bad "manual: recorded on explicit span"

# Scenario LATIN1: a non-UTF-8 (latin1) message must survive (lossily) rather
# than being dropped. The tail after the invalid 0xE9 byte proves it.
echo "$LOGS" | grep -qF 'latin1 error' \
	&& ok "latin1: non-UTF-8 message preserved" || bad "latin1: non-UTF-8 message preserved"

# Scenario REASON: a bare string reason (no Throwable) gets a synthetic "Error"
# type so the event is not dropped by backends that key on exception.type.
echo "$LOGS" | grep -qF 'exception.type: Str(Error)' \
	&& ok "reason: synthetic Error type"    || bad "reason: synthetic Error type"
echo "$LOGS" | grep -qF 'exception.message: Str(reason path: gateway timeout)' \
	&& ok "reason: exception.message" || bad "reason: exception.message"

# Scenario ANON: an anonymous exception class name embeds a NUL. The type must
# be carried length-delimited (so it is not truncated at the NUL to a bare
# "RuntimeException@anonymous") and the NUL stripped — the file path after
# "@anonymous" proves both the length-delimited capture and the strip.
echo "$LOGS" | grep -qF 'exception.type: Str(RuntimeException@anonymous/var/www/html/public/anon.php' \
	&& ok "anon: full class name preserved" || bad "anon: full class name preserved"

# Scenario REF: a Throwable passed through a PHP reference is still captured —
# the VM dereferences the by-value argument (ZVAL_COPY_DEREF in SEND_VAR) before
# the capture sees the slot, so it reads IS_OBJECT. Regression guard.
echo "$LOGS" | grep -qF 'exception.message: Str(ref path: boom)' \
	&& ok "ref: reference Throwable captured" || bad "ref: reference Throwable captured"

# Scenario UNCAUGHT: a raw uncaught exception (no #[Trace], no oxphp_apm_error)
# surfaces automatically on the request's root SERVER span, with type, message,
# the file/line extension, and a stacktrace.
echo "$LOGS" | grep -qF 'exception.message: Str(uncaught path: gateway down)' \
	&& ok "uncaught: message on root span" || bad "uncaught: message on root span"
echo "$LOGS" | grep -qF 'exception.file: Str(/var/www/html/public/uncaught.php)' \
	&& ok "uncaught: exception.file" || bad "uncaught: exception.file"
echo "$LOGS" | grep -qE 'exception\.line: (Int|Str)\(3\)' \
	&& ok "uncaught: exception.line" || bad "uncaught: exception.line"
echo "$LOGS" | grep -qF 'processPayment()' \
	&& ok "uncaught: stacktrace frame" || bad "uncaught: stacktrace frame"

# Scenario FATAL: a classless fatal (E_USER_ERROR, not a Throwable) still yields
# a located event — synthetic type + message + file/line, no stacktrace.
echo "$LOGS" | grep -qF 'exception.message: Str(fatal path: kaboom)' \
	&& ok "fatal: message captured" || bad "fatal: message captured"
echo "$LOGS" | grep -qF 'exception.type: Str(E_USER_ERROR)' \
	&& ok "fatal: synthetic type" || bad "fatal: synthetic type"

# Scenario CHAINED: an outer DomainException wrapping a PDOException cause. PHP
# renders the chain root-cause-first, so the span must bucket on the THROWN
# DomainException (type + message), not the root cause, with the location NOT
# glued into the message and the full chain kept in the stacktrace.
echo "$LOGS" | grep -qF 'exception.type: Str(DomainException)' \
	&& ok "chained: type is thrown class (not root cause)" || bad "chained: type is thrown class (not root cause)"
echo "$LOGS" | grep -qF 'exception.message: Str(chained outer: api failed)' \
	&& ok "chained: message is thrown, no glued location" || bad "chained: message is thrown, no glued location"
echo "$LOGS" | grep -qF 'Next DomainException: chained outer: api failed' \
	&& ok "chained: full chain in stacktrace" || bad "chained: full chain in stacktrace"
echo "$LOGS" | grep -qF 'exception.file: Str(/var/www/html/public/chained.php)' \
	&& ok "chained: file is thrown site" || bad "chained: file is thrown site"

# Scenario FORGE: the exception's message forges a "\n\nNext FakeClass: …"
# segment. The structural throw-hook class (ForgeReal) must win — a text parse
# would have taken FakeClass. Proves the traditional-path class is captured from
# the engine, not the (partly user-controlled) fatal text.
echo "$LOGS" | grep -qF 'exception.type: Str(ForgeReal)' \
	&& ok "forge: structural class wins" || bad "forge: structural class wins"
echo "$LOGS" | grep -qF 'exception.type: Str(FakeClass)' \
	&& bad "forge: forged class must NOT appear" || ok "forge: forged class rejected"

# Scenario STREAM: a 5xx response commits its headers and starts streaming, THEN
# a fatal is thrown. The status is already on the wire, but the exception event
# must still reach the root span — its final errors are delivered when the stream
# closes and RequestComplete is deferred until then.
echo "$LOGS" | grep -qF 'exception.message: Str(stream fatal after headers)' \
	&& ok "stream: late fatal reaches span" || bad "stream: late fatal reaches span"

# Scenario HANDLED (negative + positive control): set_exception_handler consumed
# the exception and rendered its own 500. The positive control proves the handler
# actually ran (so a 404 / parse error can't turn the negative into a false pass),
# then the negative proves no Throwable leaked onto a span.
echo "$HANDLED_BODY" | grep -qF 'handled by app' \
	&& ok "handled: handler ran (positive control)" || bad "handled: handler ran (positive control)"
echo "$LOGS" | grep -qF 'handled path: should not appear on span' \
	&& bad "handled: exception must NOT be on span" || ok "handled: no span exception (correct)"

# Scenario WORKER: a worker-mode handler that throws is caught by the fiber
# harness before zend_exception_error, yet the root span still carries the
# exception via the C-side capture at the catch site — with the message, the
# worker file, and the throwing frame (proving class/trace/file, not just msg).
# The status assertion catches the regression where a handler-body throw returned
# 200 (ctx.handler_failed is never set on the fiber path) so the >=500 gate
# silently dropped the event — a message-only grep would not have noticed.
[ "$WORKER_BOOM_CODE" = "500" ] \
	&& ok "worker: handler-body throw returns 500" || bad "worker: handler-body throw returns 500 (got '$WORKER_BOOM_CODE')"
echo "$LOGS" | grep -qF 'exception.message: Str(worker path: handler exploded)' \
	&& ok "worker: exception on root span" || bad "worker: exception on root span"
echo "$LOGS" | grep -qF 'exception.file: Str(/var/www/html/worker.php)' \
	&& ok "worker: exception.file" || bad "worker: exception.file"
echo "$LOGS" | grep -qF 'workerBoom()' \
	&& ok "worker: stacktrace frame" || bad "worker: stacktrace frame"

# Scenario WORKER-STREAM: a worker handler commits a 5xx, streams a chunk, then
# throws a late fatal. The status is already on the wire, but the fiber capture
# must still be pulled into the error stream before the streaming teardown and
# delivered via the deferred RequestComplete, so the event reaches the span.
# (Distinct from the traditional STREAM scenario: this drives the worker path,
# whose early-return on an already-sent stream previously freed the capture.)
[ "$WORKER_STREAM_CODE" = "500" ] \
	&& ok "worker-stream: streamed 5xx status" || bad "worker-stream: streamed 5xx status (got '$WORKER_STREAM_CODE')"
echo "$LOGS" | grep -qF 'exception.message: Str(worker stream fatal after headers)' \
	&& ok "worker-stream: late fatal reaches span" || bad "worker-stream: late fatal reaches span"

# Scenario WORKER-B: /a-fail threw and parked its capture in a suspending
# shutdown function while /b-ok ran on the same single-thread worker. /b-ok only
# returns "b-ok:overlapped" once it observed /a-fail's parked marker, so this
# proves a genuine overlap (not a serialized false-green). Per-fiber save/restore
# must keep /b-ok's reset from wiping the parked capture, so /a-fail's message
# still reaches its span.
[ "$BOK_BODY" = "b-ok:overlapped" ] \
	&& ok "worker-b: /b-ok provably overlapped /a-fail's parked phase" || bad "worker-b: overlap not proven (got '$BOK_BODY')"
echo "$LOGS" | grep -qF 'exception.message: Str(scenario-b: parked capture survived)' \
	&& ok "worker-b: parked capture survived the overlap" || bad "worker-b: parked capture survived the overlap"

echo
echo "  otel_exception: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
