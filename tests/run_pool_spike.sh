#!/usr/bin/env bash
# Cross-thread fcc spike runner.
#
# Boots a multi-worker OxPHP instance, captures a closure's fcc on
# whichever worker serves the first request, then fires many more
# `invoke` requests. With crossbeam's round-robin-ish dispatch the
# invokes naturally hit different workers; we record the
# capture/invoke tid pairs and assert:
#
#   1. At least one invocation ran on a worker different from the
#      capturer (cross_thread=yes) — without this the spike proves
#      nothing about the cross-thread question.
#   2. Every cross-thread invocation returned the expected value
#      `spike-value-42` (i.e. zend_call_known_function honoured the
#      foreign-thread fcc and produced correct output).
#   3. The server survived (final `reset` + `invoke` still serve),
#      i.e. no crash in the fcc invocation path.
#
# If 1 fails → spike is inconclusive (all requests happened on one
# thread). Re-run with more workers or more iterations.
#
# If 2 or 3 fails → fcc is NOT safely cross-thread-invokable; Pool
# factory path must use per-thread function-name resolution.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

PROFILE_NAME="${PROFILE_NAME:-oxphp-main}"
COMPOSE_FILES=(
    -f "$SCRIPT_DIR/compose.yml"
    -f "$SCRIPT_DIR/compose.default.yml"
)
ITERATIONS="${ITERATIONS:-20}"

cleanup() {
    docker compose -p "$PROFILE_NAME" "${COMPOSE_FILES[@]}" down --remove-orphans >/dev/null 2>&1 || true
}
trap cleanup EXIT

echo "[spike] rebuilding default profile image..."
docker compose -p "$PROFILE_NAME" "${COMPOSE_FILES[@]}" build >/dev/null

# Force multi-worker mode — the spike is meaningless with 1 worker.
export PHP_WORKERS="4"

echo "[spike] starting container with PHP_WORKERS=$PHP_WORKERS..."
docker compose -p "$PROFILE_NAME" "${COMPOSE_FILES[@]}" up -d

NAME="${PROFILE_NAME}-oxphp-default-1"
for _ in 1 2 3 4 5 6 7 8 9 10; do
    PORT=$(docker port "$NAME" 80/tcp 2>/dev/null | head -1 | awk -F: '{print $NF}') || PORT=""
    [[ -n "$PORT" ]] && break
    sleep 1
done
if [[ -z "${PORT:-}" ]]; then
    echo "FAIL: could not resolve container port"
    docker logs "$NAME" | tail -20
    exit 1
fi
echo "[spike] container port=$PORT"

# Wait for health.
for _ in 1 2 3 4 5 6 7 8 9 10; do
    if curl -fsS --max-time 2 "http://127.0.0.1:$PORT/tests/shared/test_pool_spike_fcc.php?op=reset" >/dev/null 2>&1; then
        break
    fi
    sleep 1
done

URL="http://127.0.0.1:$PORT/tests/shared/test_pool_spike_fcc.php"

echo "[spike] capturing fcc on whichever worker serves us first..."
CAPTURE_LINE=$(curl -fsS --max-time 5 "$URL?op=capture")
CAP_TID=$(printf "%s" "$CAPTURE_LINE" | awk '/tid=/ {match($0, /tid=[0-9]+/); print substr($0, RSTART+4, RLENGTH-4)}')
if [[ -z "$CAP_TID" ]]; then
    echo "FAIL: could not parse capture tid from '$CAPTURE_LINE'"
    exit 1
fi
echo "[spike] captured on tid=$CAP_TID"

echo "[spike] running $ITERATIONS invoke requests..."
cross_thread_success=0
same_thread_success=0
failures=0
for i in $(seq 1 "$ITERATIONS"); do
    LINE=$(curl -fsS --max-time 5 "$URL?op=invoke" || echo "CURL_FAIL")
    if ! printf "%s" "$LINE" | grep -q "result=spike-value-42"; then
        echo "    iter $i: UNEXPECTED: $LINE"
        failures=$((failures + 1))
        continue
    fi
    if printf "%s" "$LINE" | grep -q "cross_thread=yes"; then
        cross_thread_success=$((cross_thread_success + 1))
    else
        same_thread_success=$((same_thread_success + 1))
    fi
done

echo "[spike] results: cross-thread OK=$cross_thread_success, same-thread OK=$same_thread_success, failed=$failures"
echo "[spike] tail of server logs (last 10 lines):"
docker logs "$NAME" 2>&1 | tail -10

# Decision gate.
if [[ "$failures" -gt 0 ]]; then
    echo "SPIKE RESULT: FAIL — cross-thread fcc invocation crashed or returned wrong value $failures time(s)"
    exit 2
fi
if [[ "$cross_thread_success" -eq 0 ]]; then
    echo "SPIKE RESULT: INCONCLUSIVE — no cross-thread invocation happened (all requests stayed on capturer). Re-run with more ITERATIONS or workers."
    exit 3
fi
echo "SPIKE RESULT: PASS — $cross_thread_success cross-thread invocations succeeded with correct return value. zend_call_known_function on foreign-thread fcc is safe."
