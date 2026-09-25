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
# Scenario TRACE-CB: oxphp_apm_trace($name, $callback) runs the callback inside a
# span it owns for the callback's whole lifetime — the returning half exports a
# span carrying the $attributes argument plus whatever the callback set through
# the span id it was handed, and the throwing half exports one marked Error with
# exception.type and exception.message but deliberately no exception.stacktrace.
# The throwing half is asserted twice, once per throwable hierarchy: Exception
# and Error declare their own protected $message and neither derives from the
# other, so a capture helper reading that property with the wrong scope keeps
# working for one hierarchy while silently losing the message for the other.
#
# Root-span auto-capture scenarios: UNCAUGHT (raw uncaught exception), FATAL
# (classless E_USER_ERROR), CHAINED (outer wraps a cause — must bucket on the
# thrown class, not the root cause), FORGE (a message forging a "\n\nNext
# FakeClass: …" segment — the structural throw-hook class must win over the text
# parse), STALE-CLASS (throw+catch one class then die from a different uncaught
# one — the escaped class must win over the throw-hook snapshot of the caught
# one), STREAM (a 5xx that starts streaming then throws a late fatal — the
# status ships but the late fatal is a documented streaming boundary, NOT
# attached to the span), HANDLED (set_exception_handler swallows — no event, with
# a positive control), WORKER (fiber-catch C capture — a handler-body throw must
# return 500 AND carry class / trace / file / line), WORKER-STREAM (a worker
# handler that commits a 5xx, streams, then throws — status 500 on the wire, the
# late fatal being the same documented streaming boundary), WORKER-SHADOW (a
# worker handler throws AND a shutdown function raises its own error recorded
# first — the span must report the handler killer, not the shadowing shutdown
# error), and WORKER-B (a request parked in a suspending shutdown function keeps
# its capture while another request runs on the same worker thread — the
# per-fiber save/restore guard).
#
# A streaming/finish_request response that commits a 5xx and then throws a fatal
# *after* its headers are on the wire does NOT carry that fatal on the root span:
# RequestComplete dispatches synchronously (immediate access log / metrics) and
# the post-header errors are dropped at teardown. STREAM / WORKER-STREAM assert
# this boundary (status ships; the late-fatal message is absent from any span).
#
# Assertion is against an OpenTelemetry collector's debug exporter (stdout).
#
# Not part of run_all.sh: CI runs it from .github/workflows/e2e.yml against a
# dev image built for each supported PHP version. Run it locally after touching
# the exception-capture path (ext/bridge/oxphp_bridge.c, ext/oxphp_fiber.c,
# ext/oxphp_sapi.c, src/php/unhandled_exception.rs, src/plugins/ox_apm,
# src/plugins/ox_otel) rather than waiting for that run.
#
# The collector image is pinned because the assertions grep its debug
# exporter's text output, whose format is not a stable interface; curl is
# pinned alongside it so a run is reproducible.
#
# Usage: tests/otel_exception.sh [IMAGE_REF]   (default: oxphp-oxphp:latest)
set -u

IMAGE="${1:-oxphp-oxphp:latest}"
COLLECTOR_IMAGE="otel/opentelemetry-collector:0.161.0"
CURL_IMAGE="curlimages/curl:8.22.0"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FIX="$ROOT/tests/fixtures/otel_exception"
# Suffixed with the PID so two runs on one Docker host (two checkouts, two
# sessions) do not remove each other's containers and network.
NET="oxexc-net-$$"
COL="oxexc-col-$$"
SRV="oxexc-srv-$$"
WRK="oxexc-wrk-$$"
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
	"$COLLECTOR_IMAGE" --config /etc/otelcol/config.yaml >/dev/null

docker run -d --name "$SRV" --network "$NET" \
	-v "$FIX/auto.php":/var/www/html/public/auto.php:ro \
	-v "$FIX/manual.php":/var/www/html/public/manual.php:ro \
	-v "$FIX/trace_cb.php":/var/www/html/public/trace_cb.php:ro \
	-v "$FIX/latin1.php":/var/www/html/public/latin1.php:ro \
	-v "$FIX/reason.php":/var/www/html/public/reason.php:ro \
	-v "$FIX/anon.php":/var/www/html/public/anon.php:ro \
	-v "$FIX/ref.php":/var/www/html/public/ref.php:ro \
	-v "$FIX/uncaught.php":/var/www/html/public/uncaught.php:ro \
	-v "$FIX/fatal.php":/var/www/html/public/fatal.php:ro \
	-v "$FIX/chained.php":/var/www/html/public/chained.php:ro \
	-v "$FIX/forge.php":/var/www/html/public/forge.php:ro \
	-v "$FIX/stale_class.php":/var/www/html/public/stale_class.php:ro \
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

# Every request is sent with a traceparent carrying a trace ID of its own, and
# the server continues that trace: the root SERVER span and every child span the
# request records carry it. That is what lets an assertion be scoped to the spans
# its own request produced instead of to the whole collector log, where every
# other scenario's spans also are. The collector is fresh per run, so fixed IDs
# cannot collide with an earlier run's.
T_AUTO=00000000000000000000000000000001
T_MANUAL=00000000000000000000000000000002
T_LATIN1=00000000000000000000000000000003
T_TRACE_CB=00000000000000000000000000000004
T_REASON=00000000000000000000000000000005
T_ANON=00000000000000000000000000000006
T_REF=00000000000000000000000000000007
T_UNCAUGHT=00000000000000000000000000000008
T_FATAL=00000000000000000000000000000009
T_CHAINED=0000000000000000000000000000000a
T_FORGE=0000000000000000000000000000000b
T_STALE=0000000000000000000000000000000c
T_STREAM=0000000000000000000000000000000d
T_HANDLED=0000000000000000000000000000000e
T_WBOOM=0000000000000000000000000000000f
T_WSTREAM=00000000000000000000000000000010
T_WOK=00000000000000000000000000000011
T_WSHADOW=00000000000000000000000000000012
T_AFAIL=00000000000000000000000000000013
T_BOK=00000000000000000000000000000014
tp() { printf 'traceparent: 00-%s-0000000000000001-01' "$1"; }

# Drive both endpoints.
docker run --rm --network "$NET" "$CURL_IMAGE" -H "$(tp "$T_AUTO")" \
	-s -o /dev/null -w "auto  HTTP %{http_code}\n" "http://$SRV:80/auto.php"
docker run --rm --network "$NET" "$CURL_IMAGE" -H "$(tp "$T_MANUAL")" \
	-s -o /dev/null -w "manual HTTP %{http_code}\n" "http://$SRV:80/manual.php"
docker run --rm --network "$NET" "$CURL_IMAGE" -H "$(tp "$T_LATIN1")" \
	-s -o /dev/null -w "latin1 HTTP %{http_code}\n" "http://$SRV:80/latin1.php"
# trace_cb's body carries the callback's return value and the re-thrown message,
# neither of which reaches the collector — capture it rather than discarding it.
TRACE_CB_BODY="$(docker run --rm --network "$NET" "$CURL_IMAGE" -H "$(tp "$T_TRACE_CB")" \
	-s "http://$SRV:80/trace_cb.php")"
echo "trace_cb body: $(echo "$TRACE_CB_BODY" | tr '\n' '|')"
docker run --rm --network "$NET" "$CURL_IMAGE" -H "$(tp "$T_REASON")" \
	-s -o /dev/null -w "reason HTTP %{http_code}\n" "http://$SRV:80/reason.php"
docker run --rm --network "$NET" "$CURL_IMAGE" -H "$(tp "$T_ANON")" \
	-s -o /dev/null -w "anon  HTTP %{http_code}\n" "http://$SRV:80/anon.php"
docker run --rm --network "$NET" "$CURL_IMAGE" -H "$(tp "$T_REF")" \
	-s -o /dev/null -w "ref   HTTP %{http_code}\n" "http://$SRV:80/ref.php"
docker run --rm --network "$NET" "$CURL_IMAGE" -H "$(tp "$T_UNCAUGHT")" \
	-s -o /dev/null -w "uncaught HTTP %{http_code}\n" "http://$SRV:80/uncaught.php"
docker run --rm --network "$NET" "$CURL_IMAGE" -H "$(tp "$T_FATAL")" \
	-s -o /dev/null -w "fatal HTTP %{http_code}\n" "http://$SRV:80/fatal.php"
docker run --rm --network "$NET" "$CURL_IMAGE" -H "$(tp "$T_CHAINED")" \
	-s -o /dev/null -w "chained HTTP %{http_code}\n" "http://$SRV:80/chained.php"
docker run --rm --network "$NET" "$CURL_IMAGE" -H "$(tp "$T_FORGE")" \
	-s -o /dev/null -w "forge HTTP %{http_code}\n" "http://$SRV:80/forge.php"
docker run --rm --network "$NET" "$CURL_IMAGE" -H "$(tp "$T_STALE")" \
	-s -o /dev/null -w "stale_class HTTP %{http_code}\n" "http://$SRV:80/stale_class.php"
STREAM_CODE="$(docker run --rm --network "$NET" "$CURL_IMAGE" -H "$(tp "$T_STREAM")" \
	-s -o /dev/null --max-time 30 -w "%{http_code}" "http://$SRV:80/stream_fatal.php")"
echo "stream_fatal HTTP $STREAM_CODE"
# Capture handled's body as a positive control — proves handled.php actually ran
# (its set_exception_handler fired) so the "no event" assertion is meaningful and
# not a false pass from a 404 / parse error.
HANDLED_BODY="$(docker run --rm --network "$NET" "$CURL_IMAGE" -H "$(tp "$T_HANDLED")" \
	-s "http://$SRV:80/handled.php")"
# Capture the worker status codes: a handler-body throw and a streamed-then-thrown
# fatal must both surface as 500. Asserting the status (not just grepping the
# message) is what catches a silently-200 worker regression that the root-span
# gate would then drop.
WORKER_BOOM_CODE="$(docker run --rm --network "$NET" "$CURL_IMAGE" -H "$(tp "$T_WBOOM")" \
	-s -o /dev/null -w "%{http_code}" "http://$WRK:80/boom")"
echo "worker /boom HTTP $WORKER_BOOM_CODE"
WORKER_STREAM_CODE="$(docker run --rm --network "$NET" "$CURL_IMAGE" -H "$(tp "$T_WSTREAM")" \
	-s -o /dev/null --max-time 30 -w "%{http_code}" "http://$WRK:80/stream-boom")"
echo "worker /stream-boom HTTP $WORKER_STREAM_CODE"

# An ordinary successful request between the throwing scenarios. It was put here
# to keep the run of 500s from /boom, /stream-boom, /shadow and /a-fail under
# WORKER_MAX_CONSECUTIVE_ERRORS, back when an uncaught handler exception counted
# towards that breaker and a trip would have handed a later request a "500 PHP
# Worker Error" from the restarting worker. Uncaught exceptions no longer count —
# only a request that comes apart does — so nothing below depends on this any
# more; it stays so the scenarios keep the request sequence they were written
# against.
docker run --rm --network "$NET" "$CURL_IMAGE" -H "$(tp "$T_WOK")" \
	-s -o /dev/null -w "worker /ok HTTP %{http_code}\n" "http://$WRK:80/ok"

# Scenario WORKER-SHADOW: the handler throws (the killer), then a shutdown
# function raises its own E_USER_ERROR. oxphp_error_cb records that shutdown error
# into REQUEST_ERRORS first (during php_call_shutdown_functions), and the fiber
# capture is pulled in only at send time — so without front-insertion the earliest
# error-level entry would be the shutdown error, shadowing the killer on the span.
WORKER_SHADOW_CODE="$(docker run --rm --network "$NET" "$CURL_IMAGE" -H "$(tp "$T_WSHADOW")" \
	-s -o /dev/null --max-time 30 -w "%{http_code}" "http://$WRK:80/shadow")"
echo "worker /shadow HTTP $WORKER_SHADOW_CODE"

# Scenario B: /a-fail throws, then parks its capture in a suspending shutdown
# function (signalled by a marker file). /b-ok deterministically spin-waits for
# that marker before returning, so it provably overlaps /a-fail's parked phase on
# the single worker thread — the parked capture must survive that overlap. No
# fixed sleep: /b-ok's cooperative wait removes the timing race entirely. The
# --max-time bounds the run if the marker never appears (a real regression).
docker run --rm --network "$NET" "$CURL_IMAGE" -H "$(tp "$T_AFAIL")" \
	-s -o /dev/null --max-time 30 "http://$WRK:80/a-fail" &
BOK_BODY="$(docker run --rm --network "$NET" "$CURL_IMAGE" -H "$(tp "$T_BOK")" \
	-s --max-time 30 "http://$WRK:80/b-ok")"
echo "a-fail(bg)/b-ok body: $BOK_BODY"
wait

# Let the batch span processor flush to the collector.
sleep 8

LOGS="$(docker logs "$COL" 2>&1)"

# Print the debug-exporter block of one span — from its "Span #" header up to the
# next one — picked by the request's trace ID and the span's name, both matched
# exactly. Every scenario's spans are in the same log, so a grep over all of it
# passes on whichever span happens to carry the string; an assertion made inside
# this block passes only on the span it names. Root SERVER spans are named
# "<method> <path>"; APM spans carry the name passed to oxphp_apm_start() /
# oxphp_apm_trace(), or the function name for #[Trace].
span_in() {
	echo "$LOGS" | awk -v tid="$1" -v want="$2" '
		function emit() { if (t && n) printf "%s", buf }
		/^Span #/ { emit(); buf = $0 "\n"; t = 0; n = 0; next }
		{
			buf = buf $0 "\n"
			v = $0
			if (sub(/^ *Trace ID +: /, "", v) && v == tid) t = 1
			v = $0
			if (sub(/^ *Name +: /, "", v) && v == want) n = 1
		}
		END { emit() }
	'
}

# Scenario AUTO — on the #[Trace] function's own span.
AUTO_SPAN="$(span_in "$T_AUTO" chargeCard)"
echo "$AUTO_SPAN" | grep -qF 'exception.type: Str(RuntimeException)' \
	&& ok "auto: exception.type"   || bad "auto: exception.type"
echo "$AUTO_SPAN" | grep -qF 'exception.message: Str(auto path: card declined)' \
	&& ok "auto: exception.message" || bad "auto: exception.message"
echo "$AUTO_SPAN" | grep -qF 'exception.stacktrace: Str(#0' \
	&& ok "auto: exception.stacktrace" || bad "auto: exception.stacktrace"
echo "$AUTO_SPAN" | grep -qF 'chargeCard()' \
	&& ok "auto: stacktrace has throwing frame" || bad "auto: stacktrace has throwing frame"

# Scenario MANUAL — on the explicit span the exception was recorded onto.
MANUAL_SPAN="$(span_in "$T_MANUAL" manual_span)"
[ -n "$MANUAL_SPAN" ] \
	&& ok "manual: recorded on explicit span" || bad "manual: recorded on explicit span"
echo "$MANUAL_SPAN" | grep -qF 'exception.type: Str(LogicException)' \
	&& ok "manual: exception.type"   || bad "manual: exception.type"
echo "$MANUAL_SPAN" | grep -qF 'exception.message: Str(manual path: bad state)' \
	&& ok "manual: exception.message" || bad "manual: exception.message"

# Scenario TRACE-CB: oxphp_apm_trace() runs the callback inside a span it owns.
# The returning half and the throwing half are asserted separately, each on its
# own span.
echo "$TRACE_CB_BODY" | grep -qF 'trace_cb returned 42' \
	&& ok "trace_cb: callback return value forwarded" || bad "trace_cb: callback return value forwarded"
echo "$TRACE_CB_BODY" | grep -qF 'trace_cb rethrew: trace cb path: out of range' \
	&& ok "trace_cb: exception re-thrown to the caller" || bad "trace_cb: exception re-thrown to the caller"

TRACE_CB_OK="$(span_in "$T_TRACE_CB" trace_cb.ok)"
[ -n "$TRACE_CB_OK" ] \
	&& ok "trace_cb: returning callback exports its span" || bad "trace_cb: returning callback exports its span"
echo "$TRACE_CB_OK" | grep -qF 'component: Str(trace-cb)' \
	&& ok "trace_cb: \$attributes land on the span" || bad "trace_cb: \$attributes land on the span"
# The callback set this through the span id it was handed, so its presence is
# what proves that id addresses this span.
echo "$TRACE_CB_OK" | grep -qF 'trace_cb.inner: Str(set-from-callback)' \
	&& ok "trace_cb: span id argument addresses this span" || bad "trace_cb: span id argument addresses this span"

TRACE_CB_BOOM="$(span_in "$T_TRACE_CB" trace_cb.boom)"
echo "$TRACE_CB_BOOM" | grep -qE 'Status code *: Error' \
	&& ok "trace_cb: throwing callback marks the span Error" || bad "trace_cb: throwing callback marks the span Error"
echo "$TRACE_CB_BOOM" | grep -qF 'exception.type: Str(RangeException)' \
	&& ok "trace_cb: exception.type" || bad "trace_cb: exception.type"
echo "$TRACE_CB_BOOM" | grep -qF 'exception.message: Str(trace cb path: out of range)' \
	&& ok "trace_cb: exception.message" || bad "trace_cb: exception.message"
# No stacktrace on this path, by design. The AUTO span is the control that keeps
# this from passing because span_in returned nothing useful.
echo "$TRACE_CB_BOOM" | grep -qF 'exception.stacktrace' \
	&& bad "trace_cb: no stacktrace on the callback path" || ok "trace_cb: no stacktrace on the callback path"
echo "$AUTO_SPAN" | grep -qF 'exception.stacktrace' \
	&& ok "trace_cb: control — span_in does surface a stacktrace when there is one" \
	|| bad "trace_cb: control — span_in does surface a stacktrace when there is one"

# Error and Exception each declare their own protected $message and neither
# derives from the other, so the boom span above — a RangeException — cannot
# show whether the Error half of PHP's throwables keeps its message.
echo "$TRACE_CB_BODY" | grep -qF 'trace_cb rethrew error-hierarchy: trace cb path: wrong type' \
	&& ok "trace_cb: Error-hierarchy exception re-thrown" || bad "trace_cb: Error-hierarchy exception re-thrown"
TRACE_CB_ERR="$(span_in "$T_TRACE_CB" trace_cb.error_hierarchy)"
echo "$TRACE_CB_ERR" | grep -qE 'Status code *: Error' \
	&& ok "trace_cb: Error-hierarchy marks the span Error" || bad "trace_cb: Error-hierarchy marks the span Error"
echo "$TRACE_CB_ERR" | grep -qF 'exception.type: Str(TypeError)' \
	&& ok "trace_cb: Error-hierarchy exception.type" || bad "trace_cb: Error-hierarchy exception.type"
echo "$TRACE_CB_ERR" | grep -qF 'exception.message: Str(trace cb path: wrong type)' \
	&& ok "trace_cb: Error-hierarchy exception.message" || bad "trace_cb: Error-hierarchy exception.message"

# Scenario LATIN1: a non-UTF-8 (latin1) message must survive (lossily) rather
# than being dropped. The tail after the invalid 0xE9 byte proves it.
span_in "$T_LATIN1" boomLatin1 | grep -qF 'latin1 error' \
	&& ok "latin1: non-UTF-8 message preserved" || bad "latin1: non-UTF-8 message preserved"

# Scenario REASON: a bare string reason (no Throwable) gets a synthetic "Error"
# type so the event is not dropped by backends that key on exception.type.
REASON_SPAN="$(span_in "$T_REASON" reason_span)"
echo "$REASON_SPAN" | grep -qF 'exception.type: Str(Error)' \
	&& ok "reason: synthetic Error type"    || bad "reason: synthetic Error type"
echo "$REASON_SPAN" | grep -qF 'exception.message: Str(reason path: gateway timeout)' \
	&& ok "reason: exception.message" || bad "reason: exception.message"

# Scenario ANON: an anonymous exception class name embeds a NUL. The type must
# be carried length-delimited (so it is not truncated at the NUL to a bare
# "RuntimeException@anonymous") and the NUL stripped — the file path after
# "@anonymous" proves both the length-delimited capture and the strip.
span_in "$T_ANON" anon_span | grep -qF 'exception.type: Str(RuntimeException@anonymous/var/www/html/public/anon.php' \
	&& ok "anon: full class name preserved" || bad "anon: full class name preserved"

# Scenario REF: a Throwable passed through a PHP reference is still captured —
# the VM dereferences the by-value argument (ZVAL_COPY_DEREF in SEND_VAR) before
# the capture sees the slot, so it reads IS_OBJECT. Regression guard.
span_in "$T_REF" ref_span | grep -qF 'exception.message: Str(ref path: boom)' \
	&& ok "ref: reference Throwable captured" || bad "ref: reference Throwable captured"

# Scenario UNCAUGHT: a raw uncaught exception (no #[Trace], no oxphp_apm_error)
# surfaces automatically on the request's root SERVER span, with type, message,
# the file/line extension, and a stacktrace.
UNCAUGHT_ROOT="$(span_in "$T_UNCAUGHT" 'GET /uncaught.php')"
echo "$UNCAUGHT_ROOT" | grep -qF 'exception.message: Str(uncaught path: gateway down)' \
	&& ok "uncaught: message on root span" || bad "uncaught: message on root span"
echo "$UNCAUGHT_ROOT" | grep -qF 'exception.file: Str(/var/www/html/public/uncaught.php)' \
	&& ok "uncaught: exception.file" || bad "uncaught: exception.file"
echo "$UNCAUGHT_ROOT" | grep -qE 'exception\.line: (Int|Str)\(3\)' \
	&& ok "uncaught: exception.line" || bad "uncaught: exception.line"
echo "$UNCAUGHT_ROOT" | grep -qF 'processPayment()' \
	&& ok "uncaught: stacktrace frame" || bad "uncaught: stacktrace frame"

# error.type on the root SERVER span (OTel HTTP semantic conventions): a failed
# 5xx request carries `error.type` set to the status code string, read straight
# off the span independently of the exception event's own type.
echo "$UNCAUGHT_ROOT" | grep -qF 'error.type: Str(500)' \
	&& ok "root span: error.type on 5xx" || bad "root span: error.type on 5xx"

# Scenario FATAL: a classless fatal (E_USER_ERROR, not a Throwable) still yields
# a located event — synthetic type + message + file/line, no stacktrace.
FATAL_ROOT="$(span_in "$T_FATAL" 'GET /fatal.php')"
echo "$FATAL_ROOT" | grep -qF 'exception.message: Str(fatal path: kaboom)' \
	&& ok "fatal: message captured" || bad "fatal: message captured"
echo "$FATAL_ROOT" | grep -qF 'exception.type: Str(E_USER_ERROR)' \
	&& ok "fatal: synthetic type" || bad "fatal: synthetic type"

# Scenario CHAINED: an outer DomainException wrapping a PDOException cause. PHP
# renders the chain root-cause-first, so the span must bucket on the THROWN
# DomainException (type + message), not the root cause, with the location NOT
# glued into the message and the full chain kept in the stacktrace.
CHAINED_ROOT="$(span_in "$T_CHAINED" 'GET /chained.php')"
echo "$CHAINED_ROOT" | grep -qF 'exception.type: Str(DomainException)' \
	&& ok "chained: type is thrown class (not root cause)" || bad "chained: type is thrown class (not root cause)"
echo "$CHAINED_ROOT" | grep -qF 'exception.message: Str(chained outer: api failed)' \
	&& ok "chained: message is thrown, no glued location" || bad "chained: message is thrown, no glued location"
echo "$CHAINED_ROOT" | grep -qF 'Next DomainException: chained outer: api failed' \
	&& ok "chained: full chain in stacktrace" || bad "chained: full chain in stacktrace"
echo "$CHAINED_ROOT" | grep -qF 'exception.file: Str(/var/www/html/public/chained.php)' \
	&& ok "chained: file is thrown site" || bad "chained: file is thrown site"

# The negative assertions below each state that a string is on no span at all,
# so they grep the whole log — absent everywhere is stricter than absent from one
# span, and it also catches the string leaking onto a later request's span. What
# a whole-log absence cannot tell apart on its own is "not recorded" from "the
# span was never exported", so each is paired with a check made on its own
# request's root span — a positive assertion where the scenario has one, a bare
# "the span is in the log" control where it does not.

# Scenario FORGE: the exception's message forges a "\n\nNext FakeClass: …"
# segment. The structural throw-hook class (ForgeReal) must win — a text parse
# would have taken FakeClass. Proves the traditional-path class is captured from
# the engine, not the (partly user-controlled) fatal text.
span_in "$T_FORGE" 'GET /forge.php' | grep -qF 'exception.type: Str(ForgeReal)' \
	&& ok "forge: structural class wins" || bad "forge: structural class wins"
echo "$LOGS" | grep -qF 'exception.type: Str(FakeClass)' \
	&& bad "forge: forged class must NOT appear" || ok "forge: forged class rejected"

# Scenario STALE-CLASS: a request throws AND catches one exception, then dies from
# a DIFFERENT uncaught one. The throw-hook snapshots every thrown class and a catch
# does not clear it, but the second (escaping) throw re-fires the hook and
# overwrites the snapshot — so the root span must report the escaped class
# (StaleClassEscaped), never the earlier caught one (StaleClassCaught). Guards the
# common intra-request path of the throw-hook stale-class window (the exotic
# residual — a terminal throw that skips the hook — is not reachable from PHP).
span_in "$T_STALE" 'GET /stale_class.php' | grep -qF 'exception.type: Str(StaleClassEscaped)' \
	&& ok "stale-class: escaped class reported" || bad "stale-class: escaped class reported"
echo "$LOGS" | grep -qF 'exception.type: Str(StaleClassCaught)' \
	&& bad "stale-class: caught class must NOT leak" || ok "stale-class: caught class did not leak"

# Scenario STREAM: a 5xx response commits its headers and starts streaming, THEN
# a fatal is thrown. The status ships, but a fatal thrown after the headers went
# out is a documented streaming boundary — NOT attached to the root span (the
# post-header errors are dropped at teardown, RequestComplete is synchronous).
[ "$STREAM_CODE" = "500" ] \
	&& ok "stream: streamed 5xx status ships" || bad "stream: streamed 5xx status ships (got '$STREAM_CODE')"
[ -n "$(span_in "$T_STREAM" 'GET /stream_fatal.php')" ] \
	&& ok "stream: root span exported (control)" || bad "stream: root span exported (control)"
echo "$LOGS" | grep -qF 'stream fatal after headers' \
	&& bad "stream: late fatal must NOT reach span (boundary)" || ok "stream: late fatal not on span (documented boundary)"

# Scenario HANDLED (negative + positive controls): set_exception_handler consumed
# the exception and rendered its own 500. The body proves the handler actually
# ran (so a 404 / parse error can't turn the negative into a false pass), the
# root span proves the request was exported, then the negative proves no
# Throwable leaked onto a span.
echo "$HANDLED_BODY" | grep -qF 'handled by app' \
	&& ok "handled: handler ran (positive control)" || bad "handled: handler ran (positive control)"
[ -n "$(span_in "$T_HANDLED" 'GET /handled.php')" ] \
	&& ok "handled: root span exported (control)" || bad "handled: root span exported (control)"
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
WORKER_ROOT="$(span_in "$T_WBOOM" 'GET /boom')"
echo "$WORKER_ROOT" | grep -qF 'exception.message: Str(worker path: handler exploded)' \
	&& ok "worker: exception on root span" || bad "worker: exception on root span"
echo "$WORKER_ROOT" | grep -qF 'exception.file: Str(/var/www/html/worker.php)' \
	&& ok "worker: exception.file" || bad "worker: exception.file"
echo "$WORKER_ROOT" | grep -qF 'workerBoom()' \
	&& ok "worker: stacktrace frame" || bad "worker: stacktrace frame"

# Scenario WORKER-STREAM: a worker handler commits a 5xx, streams a chunk, then
# throws a late fatal. The status ships, but — like the traditional STREAM
# scenario — a fatal thrown after the headers went out is a documented streaming
# boundary and is NOT attached to the span.
[ "$WORKER_STREAM_CODE" = "500" ] \
	&& ok "worker-stream: streamed 5xx status" || bad "worker-stream: streamed 5xx status (got '$WORKER_STREAM_CODE')"
[ -n "$(span_in "$T_WSTREAM" 'GET /stream-boom')" ] \
	&& ok "worker-stream: root span exported (control)" || bad "worker-stream: root span exported (control)"
echo "$LOGS" | grep -qF 'worker stream fatal after headers' \
	&& bad "worker-stream: late fatal must NOT reach span (boundary)" || ok "worker-stream: late fatal not on span (documented boundary)"

# Scenario WORKER-SHADOW: the handler threw the real killer, then a shutdown
# function raised its own E_USER_ERROR (recorded into REQUEST_ERRORS first). The
# root span must report the handler killer — front-inserted so it leads the error
# stream — not the shadowing shutdown error. Prove BOTH: the killer's message is
# on this request's root span, and the shutdown error's message is on no span.
[ "$WORKER_SHADOW_CODE" = "500" ] \
	&& ok "worker-shadow: handler-throw returns 500" || bad "worker-shadow: handler-throw returns 500 (got '$WORKER_SHADOW_CODE')"
span_in "$T_WSHADOW" 'GET /shadow' | grep -qF 'exception.message: Str(shadow handler killer)' \
	&& ok "worker-shadow: span reports the handler killer" || bad "worker-shadow: span reports the handler killer"
echo "$LOGS" | grep -qF 'shadow shutdown blew up' \
	&& bad "worker-shadow: shutdown error must NOT shadow the killer" || ok "worker-shadow: shutdown error did not shadow the killer"

# Scenario WORKER-B: /a-fail threw and parked its capture in a suspending
# shutdown function while /b-ok ran on the same single-thread worker. /b-ok only
# returns "b-ok:overlapped" once it observed /a-fail's parked marker, so this
# proves a genuine overlap (not a serialized false-green). Per-fiber save/restore
# must keep /b-ok's reset from wiping the parked capture, so /a-fail's message
# still reaches its own root span.
[ "$BOK_BODY" = "b-ok:overlapped" ] \
	&& ok "worker-b: /b-ok provably overlapped /a-fail's parked phase" || bad "worker-b: overlap not proven (got '$BOK_BODY')"
span_in "$T_AFAIL" 'GET /a-fail' | grep -qF 'exception.message: Str(scenario-b: parked capture survived)' \
	&& ok "worker-b: parked capture survived the overlap" || bad "worker-b: parked capture survived the overlap"

echo
echo "  otel_exception: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
