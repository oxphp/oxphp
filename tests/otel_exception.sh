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
# Assertion is against an OpenTelemetry collector's debug exporter (stdout).
#
# NOT wired into run_all.sh or CI (like tests/graceful_drain.sh and
# tests/cli_run.sh) — run manually after touching the exception-capture path
# (ext/bridge/oxphp_bridge.c, ext/oxphp_sapi.c, src/plugins/ox_apm).
#
# Usage: tests/otel_exception.sh [IMAGE_REF]   (default: oxphp-oxphp:latest)
set -u

IMAGE="${1:-oxphp-oxphp:latest}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FIX="$ROOT/tests/fixtures/otel_exception"
NET="oxexc-net"
COL="oxexc-col"
SRV="oxexc-srv"
PASS=0
FAIL=0

ok()  { printf '  \033[32mPASS\033[0m %s\n' "$1"; PASS=$((PASS + 1)); }
bad() { printf '  \033[31mFAIL\033[0m %s\n' "$1"; FAIL=$((FAIL + 1)); }

cleanup() {
	docker rm -f "$COL" "$SRV" >/dev/null 2>&1
	docker network rm "$NET" >/dev/null 2>&1
}
trap cleanup EXIT

docker rm -f "$COL" "$SRV" >/dev/null 2>&1
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
	-e OTEL_ENABLED=true -e OTEL_APM_ENABLED=true \
	-e OTEL_EXPORTER_OTLP_ENDPOINT=http://"$COL":4317 \
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

echo
echo "  otel_exception: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
