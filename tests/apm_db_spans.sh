#!/usr/bin/env bash
#
# Integration test for APM database auto-instrumentation.
#
# Exercises the real FFI hook path that the Rust unit tests (which feed strings
# into the pure attribute builders) cannot reach: the before/after callbacks
# reading live PDO arguments and the `$this` object handle, DSN parsing into
# connection metadata, SQL obfuscation into db.statement, the prepared-statement
# store linking execute() back to its SQL, the slow-query timing flag, and the
# db.params capture gate.
#
# Scenario: a script opens `new PDO('sqlite::memory:')` (pdo_sqlite ships in the
# base image — no external DB server), runs a direct query, a prepared
# statement, and a deliberately slow recursive-CTE query. The exported child
# spans must carry:
#   - db.statement (obfuscated — the email literal collapsed to `?`)
#   - db.operation = SELECT
#   - db.system = sqlite
#   - db.statement recovered on the prepared-statement execute span
#   - db.params = [1]   (OTEL_APM_DB_CAPTURE_PARAMS_ENABLED=true)
#   - oxphp.db.slow = true on the slow query (OTEL_APM_SLOW_QUERY_MS=1)
#
# Assertion is against an OpenTelemetry collector's debug exporter (stdout).
#
# NOT wired into run_all.sh or CI (like tests/otel_exception.sh) — run manually
# after touching the APM DB hook path (ext/bridge/oxphp_bridge.c,
# src/plugins/ox_apm/hooks, src/plugins/ox_apm/{sql,connection_meta}.rs).
#
# Usage: tests/apm_db_spans.sh [IMAGE_REF]   (default: oxphp-oxphp:latest)
set -u

IMAGE="${1:-oxphp-oxphp:latest}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FIX="$ROOT/tests/fixtures/apm_db"
NET="oxdb-net"
COL="oxdb-col"
SRV="oxdb-srv"
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
	-v "$FIX/db.php":/var/www/html/public/db.php:ro \
	-v "$FIX/reuse.php":/var/www/html/public/reuse.php:ro \
	-e OTEL_ENABLED=true -e OTEL_APM_ENABLED=true \
	-e OTEL_APM_SLOW_QUERY_MS=1 \
	-e OTEL_APM_DB_CAPTURE_PARAMS_ENABLED=true \
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

# Drive the fixtures. Capture bodies as positive controls so the span
# assertions can't false-pass off a 404 / parse error.
BODY="$(docker run --rm --network "$NET" curlimages/curl:latest \
	-s --max-time 30 "http://$SRV:80/db.php")"
echo "db.php body: $BODY"
[ "$BODY" = "ok" ] && ok "fixture ran (body=ok)" || bad "fixture ran (body=ok)"

RBODY="$(docker run --rm --network "$NET" curlimages/curl:latest \
	-s --max-time 30 "http://$SRV:80/reuse.php")"
echo "reuse.php body: $RBODY"
[ "$RBODY" = "ok" ] && ok "reuse fixture ran (body=ok)" || bad "reuse fixture ran (body=ok)"

# Let the batch span processor flush to the collector.
sleep 8

LOGS="$(docker logs "$COL" 2>&1)"

# db.statement is obfuscated — the email literal must become `?`.
echo "$LOGS" | grep -qF 'db.statement: Str(SELECT * FROM users WHERE email = ?)' \
	&& ok "query: obfuscated db.statement" || bad "query: obfuscated db.statement"

echo "$LOGS" | grep -qF 'db.operation: Str(SELECT)' \
	&& ok "query: db.operation" || bad "query: db.operation"

# db.system comes from parsing the PDO DSN and looking it up by object handle.
echo "$LOGS" | grep -qF 'db.system: Str(sqlite)' \
	&& ok "connection: db.system=sqlite" || bad "connection: db.system=sqlite"

# The prepared statement's SQL appears on the prepare span (read from args[0])
# and, for PDO, on the execute span (read from PDOStatement::queryString).
echo "$LOGS" | grep -qF 'db.statement: Str(SELECT * FROM users WHERE id = ?)' \
	&& ok "prepared: db.statement (prepare + execute)" \
	|| bad "prepared: db.statement (prepare + execute)"

# db.params captured because OTEL_APM_DB_CAPTURE_PARAMS_ENABLED=true.
echo "$LOGS" | grep -qF 'db.params: Str([1])' \
	&& ok "prepared: db.params captured" || bad "prepared: db.params captured"

# The recursive-CTE query exceeds the 1ms threshold → slow flag.
echo "$LOGS" | grep -qF 'oxphp.db.slow: Str(true)' \
	&& ok "slow: oxphp.db.slow=true" || bad "slow: oxphp.db.slow=true"

# Regression (object-handle reuse): the sensitive statement ($s1, `SELECT ssn`)
# must appear no more often than the benign one ($s2, `SELECT id`) — each on its
# own prepare + execute spans. A leak of $s1's SQL onto $s2's recycled-handle
# execute (the old handle-keyed store's bug) would make `ssn` outnumber `id`.
SSN="$(echo "$LOGS" | grep -cF 'db.statement: Str(SELECT ssn FROM secrets WHERE id = ?)')"
ID="$(echo "$LOGS" | grep -cF 'db.statement: Str(SELECT id FROM secrets WHERE id = ?)')"
echo "reuse: ssn=$SSN id=$ID (expect equal, >= 1)"
[ "$SSN" = "$ID" ] && [ "$SSN" -ge 1 ] \
	&& ok "reuse: no stale-SQL leak across recycled handle" \
	|| bad "reuse: no stale-SQL leak across recycled handle"

echo
echo "apm_db_spans: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
