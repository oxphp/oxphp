#!/usr/bin/env bash
# Verify that serving requests does not make a worker grow without bound.
#
# A worker keeps the memory its requests run on: nothing gives a request's
# allocations back at the end of one, the way a request-per-script server does.
# Anything a request does not return is therefore held for the life of the
# worker, and anything it holds per request grows without bound. Two shapes of
# request are checked — one that fatals, and one that ends normally.
#
# ── The fatal ────────────────────────────────────────────────────────────────
#
# A fatal is a bailout: a longjmp out of the frame that raised it. The request
# loop catches it and serves the next request on the same fiber, so whatever the
# interrupted call left behind stays there unless the loop releases it — and a
# fiber lives as long as its worker. Two things are left behind, and this checks
# both:
#
#   1. the VM stack the interrupted frames stood on. Left alone it runs past its
#      page and the worker takes another one every few hundred fatals, which
#      shows up as the allocator's total size growing.
#   2. what those frames were holding: the copy of the script's op_array the
#      interrupted include ran on, the symbol table behind it, and the variables
#      of every function the fatal was inside. All of it is released when a
#      request returns and a fatal never returns, so each one keeps its share
#      for the life of the worker. This does not grow the allocator's total for
#      a long time — it fills space already claimed — so it is measured against
#      a control of the same requests without the fatal.
#
# ── The ordinary request ─────────────────────────────────────────────────────
#
# A request that ends normally must cost its worker nothing at all. The shape
# that is easiest to get wrong is the one that sends no Content-Type of its own:
# the engine allocates the default and hands it to the request, expecting it
# back at a request end that worker mode does not have.
#
# Checked from outside the request harness on purpose: the signal is the whole
# worker's allocator across many requests, which no single request can observe.
#
# Usage: verify_worker_does_not_grow.sh [count] [--jsonl]
#   count    how many requests per phase (default 1500)
#   --jsonl  emit one result object per check on stdout instead of a human
#            report, for run_all.sh to fold into its report
set -euo pipefail

N=1500
JSONL=""
for arg in "$@"; do
	case "$arg" in
		--jsonl) JSONL=1 ;;
		*) N="$arg" ;;
	esac
done

# What a fatal is allowed to cost on top of its control. The control is the same
# request without the fatal, so this covers only the difference between the two
# paths; a single leaked op_array copy is an order of magnitude more than this
# for even a small script.
SLACK_PER_FATAL=64

# What an ordinary request is allowed to cost. Not a difference between two
# paths this time but the whole cost, and the whole cost of a request that
# returns everything it took is zero — so this is a rounding allowance, not a
# budget. One leaked default content type is 32 bytes.
SLACK_PER_REQUEST=8

cd "$(dirname "$0")/.."
COMPOSE="docker compose -f compose.yml -f compose.fibers.yml"

# One JSONL object per check, matching what run_profile.sh emits so the
# standalone results land in the same report as the PHP suites. In --jsonl mode
# stdout carries only these objects, so everything else goes to stderr.
emit() {
	python3 -c 'import json,sys; print(json.dumps({"test": sys.argv[1], "group": "fibers", "pass": sys.argv[2] == "1", "assertions": [], "error": sys.argv[3], "meta": {}, "profile": "fibers"}, ensure_ascii=False))' "$1" "$2" "$3"
}
ok() {
	if [ -n "$JSONL" ]; then emit "$1" 1 ""; else printf '  \033[32mPASS\033[0m %s\n' "$1"; fi
}
bad() {
	if [ -n "$JSONL" ]; then emit "$1" 0 "$2"; else printf '  \033[31mFAIL\033[0m %s: %s\n' "$1" "$2"; fi
	fail=1
}
say() { [ -n "$JSONL" ] || printf '%s\n' "$1"; }

$COMPOSE up -d --wait >&2
port="$($COMPOSE port oxphp-fibers 80 | head -1 | cut -d: -f2)"
base="http://127.0.0.1:${port}/tests/fibers"

# real: what the allocator holds from the system. used: what is live inside it.
read_mem() { curl -s "${base}/fixture_memory.php" | sed "s/.*$1=\([0-9]*\).*/\1/"; }

# One connection, N sequential requests: each is its own request, and the fiber
# they land on is the same one every time with a single worker.
fire() {
	local urls=() _
	for _ in $(seq 1 "$N"); do urls+=("$1"); done
	# `-o` binds to one URL only, so the bodies of the rest would land on stdout
	# — which in --jsonl mode is the report stream.
	curl -s "${urls[@]}" > /dev/null || true
}

fail=0

# Phase 1: the same requests without the fatal, as the control.
curl -s -o /dev/null "${base}/fixture_fatal.php"
clean_before="$(read_mem used)"
fire "${base}/fixture_fatal.php"
clean_after="$(read_mem used)"
clean_cost=$(( (clean_after - clean_before) / N ))

# Phase 2: the fatal.
real_before="$(read_mem real)"
fatal_before="$(read_mem used)"
fire "${base}/fixture_fatal.php?fatal=1"
fatal_after="$(read_mem used)"
real_after="$(read_mem real)"

# Phase 3: an ordinary request that leaves the Content-Type to the engine.
curl -s -o /dev/null "${base}/fixture_no_content_type.php"
plain_before="$(read_mem used)"
fire "${base}/fixture_no_content_type.php"
plain_after="$(read_mem used)"
plain_cost=$(( (plain_after - plain_before) / N ))

$COMPOSE down -v > /dev/null 2>&1

fatal_cost=$(( (fatal_after - fatal_before) / N ))
say "per request over ${N}: clean ${clean_cost} B, fatal ${fatal_cost} B, no Content-Type ${plain_cost} B; allocator ${real_before} -> ${real_after} B"

if [ "$real_after" -le "$real_before" ]; then
	ok "${N} fatals leave the worker's allocator where it was"
else
	bad "${N} fatals leave the worker's allocator where it was" "grew from ${real_before} to ${real_after} bytes"
fi

if [ "$fatal_cost" -le $(( clean_cost + SLACK_PER_FATAL )) ]; then
	ok "a fatal costs a worker no more than the same request without one"
else
	bad "a fatal costs a worker no more than the same request without one" \
		"${fatal_cost} B per fatal against ${clean_cost} B per clean request, over ${N} each"
fi

if [ "$plain_cost" -le "$SLACK_PER_REQUEST" ]; then
	ok "a request that sends no Content-Type of its own costs the worker nothing"
else
	bad "a request that sends no Content-Type of its own costs the worker nothing" \
		"${plain_cost} B per request over ${N}"
fi

exit "$fail"
