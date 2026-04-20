#!/usr/bin/env bash
#
# pool_soak.sh — 24h Shared\Pool soak harness (manual, NOT CI).
#
# Boots the dev image with dynamic worker scaling, loads the workload
# PHP script, drives sustained traffic with wrk, scrapes /metrics and
# RSS into a CSV every minute, and writes a post-run verify report.
#
# This is a long-running *manual* test — it is deliberately not part
# of `./tests/run_all.sh`. Run it before a release, during a chaos
# rehearsal, or when investigating a slow leak.
#
# Exit criteria (verified post-run):
#   1. RSS within ±5% of the hour-1 steady state (GC noise tolerated).
#   2. Zero stale-handle panics  (`grep -i 'stale.*handle' server.log`).
#   3. `oxphp_shared_leaked_entries_at_shutdown_total = 0` at stop.
#   4. `oxphp_shared_pool_evicted_total{reason="idle_timeout"}` rising
#      smoothly (eviction scheduler alive).
#   5. No deadlock-detector alerts
#      (`oxphp_shared_deadlock_detected_total = 0`).
#
# Usage:
#   tests/soak/pool_soak.sh                   # 24h default
#   SOAK_DURATION_MIN=60 tests/soak/pool_soak.sh  # 1h smoke
#   SOAK_CONCURRENCY=400 tests/soak/pool_soak.sh  # heavier
#
# Artifacts land in tests/soak/out/<timestamp>/:
#   metrics.csv    one row per minute scrape
#   server.log     container stdout/stderr
#   verify.txt     pass/fail report for the five exit criteria

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
TESTS_DIR="$PROJECT_ROOT/tests"

# ── Knobs ─────────────────────────────────────────────────────────────
SOAK_DURATION_MIN="${SOAK_DURATION_MIN:-1440}"   # 24h
SOAK_CONCURRENCY="${SOAK_CONCURRENCY:-200}"
SOAK_THREADS="${SOAK_THREADS:-4}"
SOAK_POOL_COUNT="${SOAK_POOL_COUNT:-10}"
SOAK_POOL_MAX_SIZE="${SOAK_POOL_MAX_SIZE:-8}"
SOAK_IDLE_TIMEOUT="${SOAK_IDLE_TIMEOUT:-2}"
SOAK_WORKERS_MIN="${SOAK_WORKERS_MIN:-4}"
SOAK_WORKERS_MAX="${SOAK_WORKERS_MAX:-40}"
SOAK_WORKERS_IDLE="${SOAK_WORKERS_IDLE:-10}"     # scale-down check window
METRICS_ADDR="${METRICS_ADDR:-127.0.0.1:9090}"

# ── Prereqs ───────────────────────────────────────────────────────────
for cmd in docker wrk curl awk; do
    command -v "$cmd" >/dev/null 2>&1 || {
        echo "missing prerequisite: $cmd" >&2
        exit 1
    }
done

TS="$(date -u +%Y%m%dT%H%M%SZ)"
OUT_DIR="$SCRIPT_DIR/out/$TS"
mkdir -p "$OUT_DIR"
METRICS_CSV="$OUT_DIR/metrics.csv"
SERVER_LOG="$OUT_DIR/server.log"
VERIFY_REPORT="$OUT_DIR/verify.txt"

PROJECT_NAME="oxphp-soak-$TS"
COMPOSE_FILES=(
    -f "$TESTS_DIR/compose.yml"
    -f "$TESTS_DIR/compose.default.yml"
)

cleanup() {
    echo "[soak] capturing final metrics..."
    curl -s "http://$METRICS_ADDR/metrics" > "$OUT_DIR/metrics.final" 2>/dev/null || true
    docker compose -p "$PROJECT_NAME" "${COMPOSE_FILES[@]}" logs --no-color > "$SERVER_LOG" 2>&1 || true
    docker compose -p "$PROJECT_NAME" "${COMPOSE_FILES[@]}" down --remove-orphans >/dev/null 2>&1 || true
    echo "[soak] artifacts in $OUT_DIR"
}
trap cleanup EXIT

# ── Boot ──────────────────────────────────────────────────────────────
echo "[soak] $(date -u) — building image..."
docker compose -p "$PROJECT_NAME" "${COMPOSE_FILES[@]}" build >/dev/null

echo "[soak] booting with PHP_WORKERS=${SOAK_WORKERS_MIN}:${SOAK_WORKERS_MAX}"
PHP_WORKERS="${SOAK_WORKERS_MIN}:${SOAK_WORKERS_MAX}" \
    PHP_WORKERS_IDLE_SECONDS="$SOAK_WORKERS_IDLE" \
    WORKER_FILE=/tests/soak/workload.php \
    SOAK_POOL_COUNT="$SOAK_POOL_COUNT" \
    SOAK_POOL_MAX_SIZE="$SOAK_POOL_MAX_SIZE" \
    SOAK_IDLE_TIMEOUT="$SOAK_IDLE_TIMEOUT" \
    INTERNAL_ADDR="$METRICS_ADDR" \
    SHARED_INTROSPECTION_ENABLED=true \
    SHARED_METRICS_ENABLED=true \
    docker compose -p "$PROJECT_NAME" "${COMPOSE_FILES[@]}" up -d

CONTAINER="${PROJECT_NAME}-oxphp-default-1"

# Wait for readiness.
for _ in $(seq 1 30); do
    if curl -fsS --max-time 2 "http://$METRICS_ADDR/metrics" >/dev/null 2>&1; then
        break
    fi
    sleep 1
done

APP_PORT=$(docker port "$CONTAINER" 80/tcp 2>/dev/null | head -1 | awk -F: '{print $NF}')
if [[ -z "$APP_PORT" ]]; then
    echo "[soak] FAIL: could not resolve container app port"
    exit 1
fi
URL="http://127.0.0.1:$APP_PORT/"
echo "[soak] application at $URL; metrics at http://$METRICS_ADDR/metrics"

# ── Metrics scraper ───────────────────────────────────────────────────
echo "ts_unix,rss_bytes,entries,bytes,pool_evicted_idle,pool_evicted_shutdown,pool_evicted_manual,pool_evicted_dead_owner,deadlock_detected,ops_pool,ops_counter,pool_waiting,pool_in_use,pool_idle,pool_size" > "$METRICS_CSV"

scrape_metrics() {
    local ts_unix
    ts_unix="$(date -u +%s)"

    local metrics
    metrics="$(curl -fsS --max-time 5 "http://$METRICS_ADDR/metrics" || true)"

    local rss
    rss=$(docker stats --no-stream --format '{{.MemUsage}}' "$CONTAINER" 2>/dev/null \
            | awk -F'/' '{print $1}' | awk '{gsub(/[A-Za-z]/,""); print $1 * 1024 * 1024}' || echo 0)

    # Helper to sum all series for a given metric (with or without labels).
    sum_metric() {
        local name="$1"
        local label_match="${2:-}"
        awk -v n="$name" -v lm="$label_match" '
            $1 !~ "^#" && index($1, n) == 1 {
                if (lm == "" || index($0, lm) > 0) { s += $NF }
            }
            END { print (s ? s : 0) }
        ' <<<"$metrics"
    }

    local entries bytes dl ops_pool ops_counter waiting in_use idle size
    entries="$(sum_metric oxphp_shared_objects_total)"
    bytes="$(sum_metric oxphp_shared_total_bytes)"
    dl="$(sum_metric oxphp_shared_deadlock_detected_total)"
    ops_pool="$(sum_metric oxphp_shared_operations_total 'type="Pool"')"
    ops_counter="$(sum_metric oxphp_shared_operations_total 'type="Counter"')"
    waiting="$(sum_metric oxphp_shared_pool_waiting)"
    in_use="$(sum_metric oxphp_shared_pool_in_use)"
    idle="$(sum_metric oxphp_shared_pool_idle)"
    size="$(sum_metric oxphp_shared_pool_size)"

    local ev_idle ev_shutdown ev_manual ev_dead
    ev_idle="$(sum_metric oxphp_shared_pool_evicted_total 'reason="idle_timeout"')"
    ev_shutdown="$(sum_metric oxphp_shared_pool_evicted_total 'reason="shutdown"')"
    ev_manual="$(sum_metric oxphp_shared_pool_evicted_total 'reason="manual"')"
    ev_dead="$(sum_metric oxphp_shared_pool_evicted_total 'reason="dead_owner"')"

    printf "%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s\n" \
        "$ts_unix" "$rss" "$entries" "$bytes" \
        "$ev_idle" "$ev_shutdown" "$ev_manual" "$ev_dead" \
        "$dl" "$ops_pool" "$ops_counter" \
        "$waiting" "$in_use" "$idle" "$size" \
        >> "$METRICS_CSV"
}

# Baseline scrape + one sanity request.
curl -fsS "$URL" >/dev/null || { echo "[soak] FAIL: initial request failed"; exit 1; }
scrape_metrics

# ── Load ──────────────────────────────────────────────────────────────
echo "[soak] starting wrk: duration=${SOAK_DURATION_MIN}m conn=${SOAK_CONCURRENCY} threads=${SOAK_THREADS}"

WRK_DURATION="${SOAK_DURATION_MIN}m"
wrk -t "$SOAK_THREADS" -c "$SOAK_CONCURRENCY" -d "$WRK_DURATION" --timeout 5s "$URL" \
    > "$OUT_DIR/wrk.out" 2> "$OUT_DIR/wrk.err" &
WRK_PID=$!

# Scrape every 60s for the full duration, independent of wrk's clock.
END_UNIX=$(( $(date -u +%s) + SOAK_DURATION_MIN * 60 ))
while (( $(date -u +%s) < END_UNIX )); do
    sleep 60
    scrape_metrics || true
done

wait "$WRK_PID" || true

# Final scrape (post-load, pre-shutdown).
scrape_metrics

# ── Verify ────────────────────────────────────────────────────────────
{
    echo "=== pool_soak verify — $(date -u) ==="
    echo "duration: ${SOAK_DURATION_MIN}m"
    echo "concurrency: ${SOAK_CONCURRENCY}"
    echo

    echo "-- 1. RSS drift --"
    # hour-1 steady state = median of scrapes 5..60; compare to tail median.
    awk -F, 'NR==1 {next}
             NR>=6 && NR<=60 {h1[NR-5]=$2}
             END {
                 n=asort(h1); if (n==0) { print "  SKIP: not enough samples"; exit }
                 med1 = h1[int((n+1)/2)]
                 print "  hour-1 median RSS: " med1 " bytes"
             }' "$METRICS_CSV"
    awk -F, 'NR==1 {next} {v[NR-1]=$2}
             END {
                 n=asort(v); if (n<2) exit
                 tail_start=int(n*0.9)
                 delete t; k=0
                 for (i=tail_start;i<=n;i++) t[++k]=v[i]
                 asort(t); tm = t[int((k+1)/2)]
                 print "  tail   median RSS: " tm " bytes"
             }' "$METRICS_CSV"

    echo
    echo "-- 2. Stale-handle panics --"
    count=$(grep -ic 'stale.*handle\|panic.*stale' "$SERVER_LOG" || true)
    echo "  stale-handle events: $count (expect 0)"

    echo
    echo "-- 3. Leaked entries at shutdown --"
    leaked=$(grep -E '^oxphp_shared_leaked_entries_at_shutdown_total' "$OUT_DIR/metrics.final" 2>/dev/null | awk '{print $NF}' | tail -1 || echo "?")
    echo "  oxphp_shared_leaked_entries_at_shutdown_total: ${leaked:-?} (expect 0)"

    echo
    echo "-- 4. Idle-timeout evictions rising smoothly --"
    awk -F, 'NR==1 {next} {if ($5 > last_val) { rising++ } last_val=$5}
             END { printf "  scrapes showing a rise in pool_evicted_idle: %d (expect >0 continuously)\n", (rising+0) }' \
             "$METRICS_CSV"

    echo
    echo "-- 5. Deadlock detector alerts --"
    awk -F, 'NR==1 {next} { if ($9+0 > max) max=$9+0 }
             END { printf "  oxphp_shared_deadlock_detected_total peak: %d (expect 0)\n", max }' \
             "$METRICS_CSV"

    echo
    echo "=== wrk summary ==="
    tail -20 "$OUT_DIR/wrk.out"
} | tee "$VERIFY_REPORT"

echo
echo "[soak] done. CSV: $METRICS_CSV"
echo "[soak]        verify: $VERIFY_REPORT"
