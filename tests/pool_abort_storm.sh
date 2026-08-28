#!/usr/bin/env bash
# Abort-storm rig: does the pool still serve after rounds of clients hanging up
# mid-request, with a worker whose functions are observed?
#
# Each hangup ends its request in a fatal, and a fatal returns from nothing —
# every observed call it abandons is left open on the chain the engine keeps per
# fiber. What that costs, and why the profiler has to be on for the rig to mean
# anything, is in the worker's own bailout recovery.
#
# Not part of ./tests/run_all.sh: a round is ten seconds and the pool does not
# fail on every run, so this is a rig to reach for when the pool stops draining
# its queue, not a check to gate a change on. What it asserts each round:
#
#   1. a plain request is still answered, and answered quickly;
#   2. the queue is back to empty in the lull after the storm;
#   3. every worker's request-fiber count is back to zero;
#   4. the workers have handled everything the server took in.
#
# All four decide the round, none of them is decoration: a pool with one worker
# of four wedged still answers, so a round that reads seconds where it read
# milliseconds, or a backlog that no longer closes, is the shape the failure
# arrives in before the pool stops answering altogether. A scrape that comes
# back with nothing fails the round too — the wedge takes enough of the process
# with it that reading a missing metric as a zero would report the worst state
# as the healthiest.
#
# Usage: pool_abort_storm.sh <image> [rounds]
#   e.g. pool_abort_storm.sh oxphp-oxphp:latest 20
#
# Needs `oha` on the host (brew install oha).
set -u

IMAGE="${1:-oxphp-oxphp:latest}"
ROUNDS="${2:-20}"
CONC="${CONC:-240}"
DUR="${DUR:-8s}"
WORKERS="${WORKERS:-4}"
# Seconds. The probe answers in single-digit milliseconds on a pool that came
# back, so this is not a percentile to tune — it is the line between that and a
# request that had to wait for a worker.
SLOW_MAX="${SLOW_MAX:-2}"
# Per-invocation, like the ports: this rig runs for minutes, and a second run —
# a developer's beside one already going — must not docker rm the first one's
# container out from under it.
NAME="${NAME:-oxphp-abort-storm-$$}"
free_port() { python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()'; }
PORT_APP="${PORT_APP:-$(free_port)}"
PORT_INT="${PORT_INT:-$(free_port)}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

command -v oha >/dev/null || { echo "oha is not installed (brew install oha)"; exit 1; }

cleanup() { docker rm -f "$NAME" >/dev/null 2>&1 || true; }
trap cleanup EXIT

docker rm -f "$NAME" >/dev/null 2>&1
docker run -d --name "$NAME" \
    --cpus "${CPUS:-4}" \
    -p "${PORT_APP}:80" -p "${PORT_INT}:9090" \
    -e LOG_LEVEL=info \
    -e INTERNAL_ADDR=0.0.0.0:9090 \
    -e WORKER_FILE=/var/www/html/worker/entry.php \
    -e PHP_WORKERS="$WORKERS" \
    -e TOKIO_WORKERS=2 \
    -e EXECUTOR=sapi \
    -e PROFILER_ENABLED=true \
    -v "${SCRIPT_DIR}/fixtures/storm:/var/www/html/worker" \
    "$IMAGE" >/dev/null || exit 1

for _ in $(seq 60); do
    code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 2 "http://127.0.0.1:${PORT_INT}/health" 2>/dev/null)"
    [ "$code" = "200" ] && break
    sleep 1
done
[ "${code:-}" = "200" ] || { echo "server never came up"; docker logs "$NAME" 2>&1 | tail -20; exit 1; }

probe() { curl -s -o /dev/null -w '%{http_code}/%{time_total}' --max-time 10 "http://127.0.0.1:${PORT_APP}/"; }

# The fiber census is per worker, so the gauge comes back as one line each and
# is collected into a list rather than a single number.
#
# A field the scrape did not carry is not a zero: an empty value would pass
# every check below on its way through the shell's defaulting, so the whole
# snapshot says NOMETRICS instead and the round fails on it.
snap() {
    curl -s --max-time 5 "http://127.0.0.1:${PORT_INT}/metrics" | awk '
        /^oxphp_queue_depth /                   {d=$2}
        /^oxphp_workers_idle /                  {i=$2}
        /^oxphp_busy_workers /                  {b=$2}
        /^oxphp_worker_recycles_total /         {r=$2}
        /^oxphp_requests_total /                {rt=$2}
        /^oxphp_worker_requests_handled_total / {wh=$2}
        /^oxphp_worker_request_fibers_active\{/ {f = f $2 ","; if ($2+0 > 0) leaked=1}
        END {
            if (d == "" || rt == "" || wh == "" || f == "") { printf "NOMETRICS"; exit }
            printf "depth=%s idle=%s busy=%s recycles=%s lag=%d fibers=[%s] %s", d,i,b,r,(rt-wh),f,(leaked?"LEAKED":"")
        }'
}

echo "image:  $IMAGE"
echo "storm:  $ROUNDS rounds of -c $CONC -z $DUR --disable-keepalive, $WORKERS workers"
echo "round 0 (baseline): probe=$(probe) | $(snap)"

# Every reason a round failed, not the first one found: which of them come
# together is what separates a pool that is going under from one that already is.
note() { why="${why:+$why; }$1"; }

fail=0
for r in $(seq 1 "$ROUNDS"); do
    oha -c "$CONC" -z "$DUR" --no-tui --disable-keepalive "http://127.0.0.1:${PORT_APP}/" >/dev/null 2>&1
    sleep 2
    p="$(probe)"
    s="$(snap)"
    echo "round $r: probe=$p | $s"

    code="${p%%/*}"
    secs="${p##*/}"
    why=""
    [ "$code" = "200" ] || note "the probe answered $code"
    awk -v t="$secs" -v m="$SLOW_MAX" 'BEGIN{exit !(t+0 > m+0)}' && note "the probe took ${secs}s"

    case "$s" in
        *NOMETRICS*)
            note "the metrics scrape came back without the counters"
            ;;
        *)
            depth="$(printf '%s' "$s" | sed -n 's/.*depth=\([0-9]*\) .*/\1/p')"
            lag="$(printf '%s' "$s" | sed -n 's/.*lag=\(-*[0-9]*\) .*/\1/p')"
            [ "$depth" = "0" ] || note "the queue still holds $depth"
            [ "$lag" = "0" ] || note "the workers are $lag behind what the server took in"
            case "$s" in *LEAKED*) note "a worker still carries a request fiber" ;; esac
            ;;
    esac

    if [ -n "$why" ]; then
        echo "!!! the pool did not come back after round $r: $why"
        # Kept out of the working tree, and kept after the run: this is the
        # evidence the rig exists to collect, so it outlives the container.
        out="$(mktemp -d)"
        curl -s "http://127.0.0.1:${PORT_INT}/metrics" > "$out/metrics.txt"
        docker logs "$NAME" > "$out/logs.txt" 2>&1
        docker stats --no-stream "$NAME" >> "$out/logs.txt" 2>&1
        echo "    metrics: $out/metrics.txt   logs: $out/logs.txt"
        fail=1
        break
    fi
done

[ "$fail" = "0" ] && echo "PASS: the pool served and emptied after each of $ROUNDS rounds"
exit "$fail"
