#!/usr/bin/env bash
# Run wrk N times against a URL and report averaged metrics.
# Usage: scripts/bench-wrk.sh [URL]
# Env:   RUNS=10  THREADS=2  CONNS=220  DURATION=30s

set -euo pipefail

URL="${1:-http://localhost}"
RUNS="${RUNS:-10}"
THREADS="${THREADS:-2}"
CONNS="${CONNS:-50}"
DURATION="${DURATION:-30s}"

if ! command -v wrk >/dev/null 2>&1; then
    echo "Error: wrk not found in PATH (try: brew install wrk)" >&2
    exit 1
fi

WORKDIR="$(mktemp -d -t oxphp-bench-XXXXXX)"
trap 'rm -rf "$WORKDIR"' EXIT

echo "wrk -t${THREADS} -c${CONNS} -d${DURATION} --latency ${URL} — ${RUNS} runs"
echo

for i in $(seq 1 "$RUNS"); do
    out="$WORKDIR/run-$i.txt"
    if ! wrk -t"$THREADS" -c"$CONNS" -d"$DURATION" --latency "$URL" >"$out" 2>&1; then
        echo "wrk failed on run $i:" >&2
        cat "$out" >&2
        exit 1
    fi
    rps=$(awk '/^Requests\/sec:/ { print $2 }' "$out")
    p99=$(awk '/^[[:space:]]+99%[[:space:]]/ { print $2 }' "$out")
    printf '  [%2d/%d]  rps=%-10s  p99=%s\n' "$i" "$RUNS" "$rps" "$p99"
done

echo
echo "=== Averaged across $RUNS runs ==="

awk '
function to_us(v,    n, u) {
    if (match(v, /^[0-9.]+/)) {
        n = substr(v, RSTART, RLENGTH) + 0
        u = substr(v, RSTART + RLENGTH)
        if (u == "us") return n
        if (u == "ms") return n * 1000
        if (u == "s")  return n * 1000000
        if (u == "m")  return n * 60000000
    }
    return 0
}
function fmt_us(us) {
    if (us >= 1000000) return sprintf("%.2f s", us / 1000000)
    if (us >= 1000)    return sprintf("%.2f ms", us / 1000)
    return sprintf("%.2f us", us)
}
function to_bytes(v,    n, u) {
    if (match(v, /^[0-9.]+/)) {
        n = substr(v, RSTART, RLENGTH) + 0
        u = substr(v, RSTART + RLENGTH)
        if (u == "B")  return n
        if (u == "KB") return n * 1024
        if (u == "MB") return n * 1024 * 1024
        if (u == "GB") return n * 1024 * 1024 * 1024
    }
    return 0
}
function fmt_bytes(b) {
    if (b >= 1024*1024*1024) return sprintf("%.2f GB/s", b / 1024 / 1024 / 1024)
    if (b >= 1024*1024)      return sprintf("%.2f MB/s", b / 1024 / 1024)
    if (b >= 1024)           return sprintf("%.2f KB/s", b / 1024)
    return sprintf("%.0f B/s", b)
}
function to_count(v,    n, u) {
    if (match(v, /^[0-9.]+/)) {
        n = substr(v, RSTART, RLENGTH) + 0
        u = substr(v, RSTART + RLENGTH)
        if (u == "")  return n
        if (u == "k") return n * 1000
        if (u == "M") return n * 1000000
    }
    return 0
}

/^[[:space:]]+Latency[[:space:]]/  { lat_avg += to_us($2); lat_max += to_us($4); n_lat++ }
/^[[:space:]]+50%[[:space:]]/      { p50 += to_us($2); n_p50++ }
/^[[:space:]]+75%[[:space:]]/      { p75 += to_us($2); n_p75++ }
/^[[:space:]]+90%[[:space:]]/      { p90 += to_us($2); n_p90++ }
/^[[:space:]]+99%[[:space:]]/      { p99 += to_us($2); n_p99++ }
/^Requests\/sec:/                  { rps_n++; rps_sum += $2; rps_sumsq += $2 * $2;
                                     if (rps_n == 1 || $2 < rps_min) rps_min = $2;
                                     if ($2 > rps_max) rps_max = $2 }
/^Transfer\/sec:/                  { transfer += to_bytes($2); n_transfer++ }
/requests in/                      { reqs += $1; n_reqs++ }

END {
    if (rps_n) {
        mean = rps_sum / rps_n
        var  = (rps_sumsq / rps_n) - mean * mean
        if (var < 0) var = 0
        sd   = sqrt(var)
        cv   = (mean > 0) ? (sd / mean) * 100 : 0
        printf "Throughput:\n"
        printf "  Requests/sec:  %10.2f  (± %.2f, cv=%.1f%%)\n", mean, sd, cv
        printf "  min / max:     %10.2f / %.2f\n", rps_min, rps_max
    }
    if (n_transfer) {
        printf "  Transfer/sec:  %10s\n", fmt_bytes(transfer / n_transfer)
    }
    if (n_reqs) {
        printf "  Total reqs:    %10.0f  (avg per run)\n", reqs / n_reqs
    }
    if (n_lat) {
        printf "\nLatency (avg of per-run averages):\n"
        printf "  Avg:    %s\n", fmt_us(lat_avg / n_lat)
        printf "  Max:    %s\n", fmt_us(lat_max / n_lat)
    }
    if (n_p50) {
        printf "\nLatency distribution (avg of per-run percentiles):\n"
        printf "  p50:    %s\n", fmt_us(p50 / n_p50)
        printf "  p75:    %s\n", fmt_us(p75 / n_p75)
        printf "  p90:    %s\n", fmt_us(p90 / n_p90)
        printf "  p99:    %s\n", fmt_us(p99 / n_p99)
    }
}
' "$WORKDIR"/run-*.txt
