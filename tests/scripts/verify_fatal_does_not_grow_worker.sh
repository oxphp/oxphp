#!/usr/bin/env bash
# Verify that fatals do not make a worker grow without bound.
#
# A fatal is a bailout: a longjmp out of the frame that raised it. The request
# loop catches it and serves the next request on the same fiber, so whatever the
# interrupted call left on that fiber's VM stack stays there unless the loop
# rewinds it — and a fiber lives as long as its worker. Left alone, the stack
# runs past its page and the worker takes another one, for every few hundred
# fatals, forever.
#
# Checked from outside the request harness on purpose: the signal is the whole
# worker's allocator across many requests, which no single request can observe.
#
# Usage: verify_fatal_does_not_grow_worker.sh [count]
set -euo pipefail

N="${1:-1500}"
cd "$(dirname "$0")/.."
COMPOSE="docker compose -f compose.yml -f compose.fibers.yml"

$COMPOSE up -d --wait
port="$($COMPOSE port oxphp-fibers 80 | head -1 | cut -d: -f2)"
base="http://127.0.0.1:${port}/tests/fibers"

read_real() { curl -s "${base}/fixture_memory.php" | sed 's/.*real=\([0-9]*\).*/\1/'; }

before="$(read_real)"

# One connection, N sequential requests: each fatal is its own request, and the
# fiber they land on is the same one every time with a single worker.
urls=()
for _ in $(seq 1 "$N"); do urls+=("${base}/fixture_fatal.php?fatal=1"); done
curl -s -o /dev/null "${urls[@]}" || true

after="$(read_real)"
$COMPOSE down -v > /dev/null 2>&1

echo "worker allocator after ${N} fatals: ${before} -> ${after} bytes"
if [ "$after" -gt "$before" ]; then
  echo "FAIL: the worker took more memory from the allocator across ${N} fatals"
  exit 1
fi
echo "PASS: ${N} fatals left the worker's allocator where it was"
