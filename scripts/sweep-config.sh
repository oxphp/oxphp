#!/usr/bin/env bash
# Sweep (TOKIO_WORKERS × PHP_WORKERS × CONNS) and emit Pareto frontier (RPS vs p99).
# Restarts the docker-compose service for each (tokio, php) combination.
#
# Usage:   scripts/sweep-config.sh
# Env:
#   TOKIO_LIST      space-separated TOKIO_WORKERS values        (default: "1 2 4")
#   PHP_LIST        space-separated PHP_WORKERS values          (default: "1 2 4 8")
#   CONNS_LIST      space-separated wrk -c values               (default: "8 64 256")
#   THREADS         wrk -t                                      (default: 2)
#   DURATION        wrk -d                                      (default: 15s)
#   RUNS            wrk repetitions per combo                   (default: 3)
#   URL             target URL                                  (default: http://localhost)
#   SERVICE         compose service name                        (default: oxphp)
#   COMPOSE_FILE    path to compose.yml                         (default: compose.yml)
#   WAIT_TIMEOUT    seconds to wait for server readiness        (default: 30)
#   WARMUP_SECONDS  warmup wrk before each measurement          (default: 3)
#   RESULTS_TSV     where to write the raw TSV                  (default: /tmp/...)

set -euo pipefail

TOKIO_LIST="${TOKIO_LIST:-1 2 4}"
PHP_LIST="${PHP_LIST:-1 2 4 8}"
CONNS_LIST="${CONNS_LIST:-8 64 256}"
THREADS="${THREADS:-2}"
DURATION="${DURATION:-15s}"
RUNS="${RUNS:-3}"
URL="${URL:-http://localhost}"
SERVICE="${SERVICE:-oxphp}"
COMPOSE_FILE="${COMPOSE_FILE:-compose.yml}"
WAIT_TIMEOUT="${WAIT_TIMEOUT:-30}"
WARMUP_SECONDS="${WARMUP_SECONDS:-3}"
RESULTS_TSV="${RESULTS_TSV:-/tmp/oxphp-sweep-$(date +%Y%m%d-%H%M%S).tsv}"

for cmd in docker wrk awk curl; do
    command -v "$cmd" >/dev/null || { echo "Error: $cmd not found in PATH" >&2; exit 1; }
done

[[ -f "$COMPOSE_FILE" ]] || { echo "Error: $COMPOSE_FILE not found" >&2; exit 1; }

OVERRIDE="$(mktemp -t oxphp-sweep-override-XXXXXX).yml"
cleanup() {
    local rc=$?
    rm -f "$OVERRIDE"
    echo >&2
    echo "Restoring default compose configuration..." >&2
    docker compose -f "$COMPOSE_FILE" up -d --force-recreate >/dev/null 2>&1 || true
    exit $rc
}
trap cleanup EXIT INT TERM

write_override() {
    cat >"$OVERRIDE" <<EOF
services:
  ${SERVICE}:
    environment:
      TOKIO_WORKERS: "$1"
      PHP_WORKERS: "$2"
EOF
}

wait_ready() {
    local deadline=$((SECONDS + WAIT_TIMEOUT))
    while [[ $SECONDS -lt $deadline ]]; do
        curl -fsS --max-time 1 "$URL" >/dev/null 2>&1 && return 0
        sleep 0.5
    done
    return 1
}

to_us() {
    awk -v v="$1" 'BEGIN {
        if (match(v, /^[0-9.]+/)) {
            n = substr(v, RSTART, RLENGTH) + 0
            u = substr(v, RSTART + RLENGTH)
            if (u == "us")      print n
            else if (u == "ms") print n * 1000
            else if (u == "s")  print n * 1000000
            else if (u == "m")  print n * 60000000
            else                print 0
        } else print 0
    }'
}

run_combo() {
    local tokio="$1" php="$2" conns="$3"
    local rps_sum=0 p99_sum=0 ok=0 rps_min="" rps_max=""
    for r in $(seq 1 "$RUNS"); do
        local out rps p99 p99_us
        out=$(wrk -t"$THREADS" -c"$conns" -d"$DURATION" --latency "$URL" 2>&1) || continue
        rps=$(awk '/^Requests\/sec:/ { print $2 }' <<<"$out")
        p99=$(awk '/^[[:space:]]+99%[[:space:]]/ { print $2 }' <<<"$out")
        [[ -z $rps || -z $p99 ]] && continue
        p99_us=$(to_us "$p99")
        rps_sum=$(awk -v a="$rps_sum" -v b="$rps" 'BEGIN { printf "%.4f", a + b }')
        p99_sum=$(awk -v a="$p99_sum" -v b="$p99_us" 'BEGIN { printf "%.4f", a + b }')
        if [[ -z $rps_min ]] || awk -v a="$rps" -v b="$rps_min" 'BEGIN { exit !(a < b) }'; then
            rps_min="$rps"
        fi
        if [[ -z $rps_max ]] || awk -v a="$rps" -v b="$rps_max" 'BEGIN { exit !(a > b) }'; then
            rps_max="$rps"
        fi
        ok=$((ok + 1))
    done
    [[ $ok -eq 0 ]] && return 1
    local rps_avg p99_avg cv
    rps_avg=$(awk -v s="$rps_sum" -v n="$ok" 'BEGIN { printf "%.2f", s/n }')
    p99_avg=$(awk -v s="$p99_sum" -v n="$ok" 'BEGIN { printf "%.2f", s/n }')
    cv=$(awk -v lo="$rps_min" -v hi="$rps_max" -v m="$rps_avg" \
        'BEGIN { if (m > 0) printf "%.1f", ((hi - lo) / m) * 100; else print "0.0" }')
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%d\n' \
        "$tokio" "$php" "$conns" "$rps_avg" "$p99_avg" "$cv" "$ok"
}

echo "Sweep matrix:" >&2
echo "  TOKIO_WORKERS  : $TOKIO_LIST" >&2
echo "  PHP_WORKERS    : $PHP_LIST" >&2
echo "  CONNS          : $CONNS_LIST" >&2
echo "  per combo      : ${RUNS} runs × ${DURATION}" >&2
echo "  results        : $RESULTS_TSV" >&2
echo >&2

printf 'tokio\tphp\tconns\trps\tp99_us\trps_cv_pct\tok_runs\n' | tee "$RESULTS_TSV"

total=0
for t in $TOKIO_LIST; do for p in $PHP_LIST; do for c in $CONNS_LIST; do
    total=$((total + 1)); done; done; done
done_n=0

for tokio in $TOKIO_LIST; do
    for php in $PHP_LIST; do
        echo >&2
        echo "→ TOKIO=$tokio PHP=$php — recreating container" >&2
        write_override "$tokio" "$php"
        if ! docker compose -f "$COMPOSE_FILE" -f "$OVERRIDE" \
                up -d --force-recreate "$SERVICE" >/dev/null 2>&1; then
            echo "  !! docker compose up failed" >&2
            done_n=$((done_n + ${#CONNS_LIST}))
            continue
        fi
        if ! wait_ready; then
            echo "  !! server not ready within ${WAIT_TIMEOUT}s — skipping" >&2
            done_n=$((done_n + ${#CONNS_LIST}))
            continue
        fi
        if [[ $WARMUP_SECONDS -gt 0 ]]; then
            wrk -t1 -c4 -d"${WARMUP_SECONDS}s" "$URL" >/dev/null 2>&1 || true
        fi
        for conns in $CONNS_LIST; do
            done_n=$((done_n + 1))
            printf '  [%2d/%d] CONNS=%-4s ' "$done_n" "$total" "$conns" >&2
            if line=$(run_combo "$tokio" "$php" "$conns"); then
                echo "$line" | tee -a "$RESULTS_TSV" >&2
            else
                echo "FAILED" >&2
            fi
        done
    done
done

echo >&2
echo "=== Pareto frontier (max RPS ↑, min p99 ↓) ===" >&2

awk -F'\t' '
NR == 1 { next }
{
    n++
    tokio[n] = $1; php[n] = $2; conns[n] = $3
    rps[n]   = $4 + 0; p99[n] = $5 + 0; cv[n] = $6
}
END {
    printf "%-6s %-5s %-7s %-12s %-12s %-8s %s\n", \
        "tokio", "php", "conns", "rps", "p99(ms)", "cv%", ""
    for (i = 1; i <= n; i++) {
        dominated = 0
        for (j = 1; j <= n; j++) {
            if (i == j) continue
            if (rps[j] >= rps[i] && p99[j] <= p99[i] && \
                (rps[j] > rps[i] || p99[j] < p99[i])) {
                dominated = 1; break
            }
        }
        marker = dominated ? "" : "★ pareto"
        printf "%-6s %-5s %-7s %-12.2f %-12.3f %-8s %s\n", \
            tokio[i], php[i], conns[i], rps[i], p99[i] / 1000, cv[i], marker
    }
}' "$RESULTS_TSV" | (read -r header; printf '%s\n' "$header"; sort -k4 -n -r) >&2

echo >&2
echo "Best by RPS:" >&2
tail -n +2 "$RESULTS_TSV" | sort -k4 -n -r | head -3 | \
    awk -F'\t' '{ printf "  TOKIO=%s PHP=%s CONNS=%s → %.0f rps, p99=%.2f ms\n", \
        $1, $2, $3, $4, $5/1000 }' >&2

echo "Best by p99 (where rps ≥ 80%% of peak):" >&2
peak=$(tail -n +2 "$RESULTS_TSV" | awk -F'\t' 'BEGIN{m=0} {if ($4>m) m=$4} END{print m}')
tail -n +2 "$RESULTS_TSV" | \
    awk -F'\t' -v peak="$peak" '$4 + 0 >= peak * 0.8' | \
    sort -k5 -n | head -3 | \
    awk -F'\t' '{ printf "  TOKIO=%s PHP=%s CONNS=%s → %.0f rps, p99=%.2f ms\n", \
        $1, $2, $3, $4, $5/1000 }' >&2

echo >&2
echo "Full TSV: $RESULTS_TSV" >&2
