#!/usr/bin/env bash
#
# Integration test for APM auto-instrumentation of phpredis.
#
# The APM plugin registers Redis::hget / hset / lpush / rpush in lowercase,
# while phpredis declares them as hGet / hSet / lPush / rPush. The hook wrapper
# has to match a call to its original handler without regard to case, or the
# call returns NULL and never reaches Redis.
#
# Scenario (classic request, RUNTIME_HOOKS unset): tests/fixtures/apm_db/redis.php
# writes through rPush, rpush, a first-class callable of lPush and hSet, then
# reads everything back. Two halves are asserted:
#   - the data arrived: return values and lLen / hGet read-backs are correct;
#   - the wrapper ran: the collector received a `Redis::<method>` span for each
#     hooked method. Without this half the first one would also pass with the
#     hooks not installed at all.
#
# Assertion is against an OpenTelemetry collector's debug exporter (stdout).
#
# NOT wired into run_all.sh or CI (like tests/apm_db_spans.sh) — run manually
# after touching the APM hook wrapper (ext/bridge/oxphp_bridge.c) or
# src/plugins/ox_apm/hooks.
#
# The image must carry phpredis; build one from tests/fixtures/hooks_db:
#   docker build --build-arg BASE_IMAGE=oxphp-oxphp:latest \
#     -t oxphp-hooksdb:latest tests/fixtures/hooks_db
#
# Usage: tests/apm_redis_spans.sh [IMAGE_REF]   (default: oxphp-hooksdb:latest)
set -u

IMAGE="${1:-oxphp-hooksdb:latest}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FIX="$ROOT/tests/fixtures/apm_db"
# Suffixed with the PID so two runs on one Docker host do not remove each
# other's containers and network.
NET="oxredis-net-$$"
COL="oxredis-col-$$"
RDS="oxredis-rds-$$"
SRV="oxredis-srv-$$"
PASS=0
FAIL=0

ok()  { printf '  \033[32mPASS\033[0m %s\n' "$1"; PASS=$((PASS + 1)); }
bad() { printf '  \033[31mFAIL\033[0m %s\n' "$1"; FAIL=$((FAIL + 1)); }

cleanup() {
	docker rm -f "$COL" "$RDS" "$SRV" >/dev/null 2>&1
	docker network rm "$NET" >/dev/null 2>&1
}
trap cleanup EXIT

cleanup
docker network create "$NET" >/dev/null

docker run -d --name "$COL" --network "$NET" \
	-v "$FIX/otelcol.yaml":/etc/otelcol/config.yaml:ro \
	otel/opentelemetry-collector:latest --config /etc/otelcol/config.yaml >/dev/null

docker run -d --name "$RDS" --network "$NET" redis:8-alpine >/dev/null

docker run -d --name "$SRV" --network "$NET" \
	-v "$FIX/redis.php":/var/www/html/public/redis.php:ro \
	-e OTEL_ENABLED=true -e OTEL_APM_ENABLED=true \
	-e OTEL_EXPORTER_OTLP_ENDPOINT=http://"$COL":4317 \
	-e INTERNAL_ADDR=0.0.0.0:9090 \
	-e LOG_LEVEL=error \
	-e DB_REDIS_HOST="$RDS" \
	"$IMAGE" >/dev/null

# Wait for the server's internal health endpoint and for Redis (max ~30s).
ready=0
for _ in $(seq 1 30); do
	if docker exec "$SRV" wget -q --spider http://127.0.0.1:9090/health 2>/dev/null &&
		docker exec "$RDS" redis-cli ping 2>/dev/null | grep -q PONG; then
		ready=1
		break
	fi
	sleep 1
done
[ "$ready" = 1 ] || { echo "server or redis did not become ready"; docker logs "$SRV" | tail -20; exit 1; }

docker exec "$SRV" php -m | grep -qx redis || { echo "image $IMAGE has no phpredis"; exit 1; }

BODY="$(docker run --rm --network "$NET" curlimages/curl:latest \
	-s --max-time 30 "http://$SRV:80/redis.php")"
echo "redis.php body: $BODY"

EXPECTED='{"rPush":1,"rpush":1,"lPush":2,"hSet":1,"hGet":"v","len1":2,"len2":1}'
[ "$BODY" = "$EXPECTED" ] \
	&& ok "writes reached Redis and read back" \
	|| bad "writes reached Redis and read back (expected $EXPECTED)"

# Let the batch span processor flush to the collector.
sleep 8

LOGS="$(docker logs "$COL" 2>&1)"

# Positive control: connect is registered in its declared spelling, so its span
# is exported whatever the wrapper does with case — a miss here means the
# export or this grep is broken, not the hooks below.
echo "$LOGS" | grep -qE "Name +: Redis::connect\$" \
	&& ok "control: span Redis::connect exported" \
	|| bad "control: span Redis::connect exported"

for m in rPush lPush hSet hGet; do
	echo "$LOGS" | grep -qE "Name +: Redis::$m\$" \
		&& ok "span Redis::$m exported" || bad "span Redis::$m exported"
done

echo
echo "apm_redis_spans: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
