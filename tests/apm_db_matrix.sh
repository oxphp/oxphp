#!/usr/bin/env bash
#
# APM database auto-instrumentation matrix — verifies the hook pipeline against
# real database servers across all three drivers:
#
#   - SQLite     (pdo_sqlite, in-memory)
#   - MySQL 9    (pdo_mysql  + mysqli)
#   - PostgreSQL 17 (pdo_pgsql)
#
# Brings up mysql:9 + postgres:17 + an OTel collector + an OxPHP server (dev
# image plus the DB client extensions) via tests/compose.apm-db.yml, drives one
# fixture per driver, and asserts the exported spans against the collector's
# debug exporter. Each fixture uses a distinct table name (t_sqlite, t_mysql,
# t_mysqli, t_pg) so its statements are unambiguous in the shared log.
#
# For each driver the exported spans must carry:
#   - db.system            (sqlite | mysql | postgresql)
#   - db.statement         obfuscated (the email literal collapsed to `?`)
#   - db.statement         on the prepare span, and on the PDO execute span too
#                          (from PDOStatement::queryString); mysqli execute has none
#   - server.address       (mysql / postgres — the DSN / ctor host)
# Cross-cutting (proven once):
#   - db.params = [1]      on a PDO execute (OTEL_APM_DB_CAPTURE_PARAMS_ENABLED)
#   - oxphp.db.slow = true on a query past OTEL_APM_SLOW_QUERY_MS
#
# NOT wired into run_all.sh or CI — heavy (pulls the DB images). Run manually
# after touching the APM DB hook path.
#
# Usage: tests/apm_db_matrix.sh
#   Requires the dev image: docker build -f docker/dev/Dockerfile -t oxphp-oxphp:latest .
set -u

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
COMPOSE="$ROOT/tests/compose.apm-db.yml"
PROJECT="oxphp-apmdb"
DC=(docker compose -p "$PROJECT" -f "$COMPOSE")
PASS=0
FAIL=0

ok()  { printf '  \033[32mPASS\033[0m %s\n' "$1"; PASS=$((PASS + 1)); }
bad() { printf '  \033[31mFAIL\033[0m %s\n' "$1"; FAIL=$((FAIL + 1)); }

cleanup() { "${DC[@]}" down -v --remove-orphans >/dev/null 2>&1; }
trap cleanup EXIT

# The extension image builds FROM the dev image.
if ! docker image inspect oxphp-oxphp:latest >/dev/null 2>&1; then
	echo "oxphp-oxphp:latest not found — build it first:"
	echo "  docker build -f docker/dev/Dockerfile -t oxphp-oxphp:latest ."
	exit 1
fi

echo "Bringing up mysql:9 + postgres:17 + collector + oxphp (this pulls DB images on first run)…"
"${DC[@]}" down -v --remove-orphans >/dev/null 2>&1
if ! "${DC[@]}" up -d --build --wait --wait-timeout 240; then
	echo "stack did not become healthy"; "${DC[@]}" ps; "${DC[@]}" logs --tail 40 oxphp
	exit 1
fi

drive() {
	local path="$1"
	local body
	body="$("${DC[@]}" exec -T oxphp wget -qO- "http://127.0.0.1:80/$path" 2>/dev/null)"
	echo "$path -> ${body:-<empty>}"
	[ "$body" = "ok" ] && ok "$path ran (body=ok)" || bad "$path ran (body=ok)"
}

drive sqlite.php
drive mysql.php
drive mysqli.php
drive pgsql.php

# Let the batch span processor flush to the collector.
sleep 8

LOGS="$("${DC[@]}" logs otelcol 2>&1)"

has() { echo "$LOGS" | grep -qF "$1"; }
check() { has "$1" && ok "$2" || bad "$2"; }

echo
echo "── SQLite ──"
check 'db.system: Str(sqlite)'                              "sqlite: db.system"
check 'db.statement: Str(SELECT * FROM t_sqlite WHERE email = ?)' "sqlite: obfuscated query"
check 'db.statement: Str(SELECT * FROM t_sqlite WHERE id = ?)'    "sqlite: prepared stmt db.statement (prepare + execute)"

echo "── MySQL 9 (PDO) ──"
check 'db.system: Str(mysql)'                               "mysql: db.system"
check 'server.address: Str(mysql)'                          "mysql: server.address from DSN"
check 'db.statement: Str(SELECT * FROM t_mysql WHERE email = ?)'  "mysql(pdo): obfuscated query"
check 'db.statement: Str(SELECT * FROM t_mysql WHERE id = ?)'     "mysql(pdo): prepared stmt db.statement (prepare + execute)"

echo "── MySQL 9 (mysqli) ──"
check 'db.statement: Str(SELECT * FROM t_mysqli WHERE email = ?)' "mysqli: obfuscated query"
check 'db.statement: Str(SELECT * FROM t_mysqli WHERE id = ?)'    "mysqli: prepared stmt db.statement (on prepare span)"

echo "── PostgreSQL 17 (PDO) ──"
check 'db.system: Str(postgresql)'                          "pgsql: db.system"
check 'server.address: Str(postgres)'                       "pgsql: server.address from DSN"
check 'db.statement: Str(SELECT * FROM t_pg WHERE email = ?)'     "pgsql: obfuscated query"
check 'db.statement: Str(SELECT * FROM t_pg WHERE id = ?)'        "pgsql: prepared stmt db.statement (prepare + execute)"

echo "── Cross-cutting ──"
check 'db.params: Str([1])'                                 "db.params captured on PDO execute"
check 'oxphp.db.slow: Str(true)'                            "slow-query flag"

echo
echo "apm_db_matrix: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
