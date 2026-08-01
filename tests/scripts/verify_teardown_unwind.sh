#!/usr/bin/env bash
# Verify that tearing down a worker with a request still parked unwinds that
# request cleanly, in the two ways a worker goes away:
#
#   1. the worker exits on its own while a request is parked mid-sleep — the
#      scheduler must unwind it before freeing the state the request runs on;
#   2. the whole process is stopped while a request is parked — it must exit on
#      its own, without a crash and without hanging until the runtime kills it.
#
# Verified from outside the container on purpose: the behaviour under test is
# what happens to a request that is still parked when its worker is torn down,
# and a request cannot assert on its own teardown.
set -euo pipefail

cd "$(dirname "$0")/.."
COMPOSE="docker compose -f compose.yml -f compose.fibers.yml"

$COMPOSE up -d --wait
port="$($COMPOSE port oxphp-fibers 80 | head -1 | cut -d: -f2)"
base="http://127.0.0.1:${port}"

fail=0

# ── 1. Worker teardown with a request parked mid-request ──────────────────
# The request schedules its own worker's exit and then parks, so the scheduler
# is torn down with this fiber suspended. It must not hang, and the worker the
# pool respawns must serve normally afterwards.
curl -s --max-time 20 "${base}/tests/fibers/fixture_long_park.php?exit=1" > /tmp/oxphp_park_exit.out
sleep 2

state="$($COMPOSE ps -a --format '{{.State}}' oxphp-fibers)"
[ "$state" = "running" ] || { echo "FAIL: container is $state after a worker exited under a parked request"; fail=1; }

after="$(curl -s -o /dev/null -w '%{http_code}' --max-time 20 "${base}/tests/fibers/test_fiber_identity.php")"
[ "$after" = "200" ] || { echo "FAIL: respawned worker answered $after (want 200)"; fail=1; }

# ── 2. Process teardown with a request parked mid-request ─────────────────
curl -s --max-time 30 "${base}/tests/fibers/fixture_long_park.php" > /tmp/oxphp_park_stop.out &
curl_pid=$!
sleep 1

start=$(date +%s)
$COMPOSE stop -t 25 oxphp-fibers
elapsed=$(( $(date +%s) - start ))

exit_code="$($COMPOSE ps -a --format '{{.ExitCode}}' oxphp-fibers)"
wait "$curl_pid" || true

log="$($COMPOSE logs oxphp-fibers 2>&1)"
$COMPOSE down -v > /dev/null 2>&1

[ "$exit_code" = "0" ] || { echo "FAIL: container exit code $exit_code (want 0)"; fail=1; }
[ "$elapsed" -lt 25 ] || { echo "FAIL: took ${elapsed}s, was killed at the stop timeout"; fail=1; }
echo "$log" | grep -qiE "segmentation fault|SIGSEGV|SIGABRT|panicked at" \
  && { echo "FAIL: crash in log"; fail=1; }
echo "$log" | grep -qiE "leak|still referenced" \
  && { echo "FAIL: leak report in log"; fail=1; }

[ "$fail" = "0" ] && echo "PASS: teardown unwound a parked request, worker exit and process stop, in ${elapsed}s"
exit "$fail"
