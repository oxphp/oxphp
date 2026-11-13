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
# Every fatal is sent between two requests that end normally. Three fatals in a
# row and the server retires the worker as one that fails every request, so a
# reading taken after a run of them is a fresh worker's and says nothing about
# what the fatals left. For the same reason every reading is checked against the
# server's count of worker replacements: with a single worker, a count that moved
# during a phase means its two readings came from different workers.
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

# What a fatal is allowed to cost on top of its control: its message, which the
# engine leaks and nothing in the worker can give back. The engine formats the
# message into a string it releases after the error callback returns, and the
# callback of a fatal bails out instead of returning; the callback cannot release
# it in the engine's place, because other callers pass a string they do not own.
# The fixtures' messages name the fixture's path and cost 160 and 192 bytes. A
# single leaked op_array copy is an order of magnitude more than this for even a
# small script.
# SYNC: php-src 8.4.27 / 8.5.12 Zend/zend.c zend_error_va_list().
ALLOWED_PER_FATAL=192

# What a fatal raised by a compile error is allowed to cost. The compile is
# abandoned partway, and what it had already built stays allocated until the
# request ends, which on a worker is when the worker goes: the op_array under
# construction, the class and method entries in the compiler's arena and, for
# an included file, its open handle. About 6.5 KB for eval'd code and 7 KB for
# an included file here. The syntax tree is the one part that can be given back,
# and the largest — its arena alone is 32 KiB.
# SYNC: php-src 8.4.27 / 8.5.12 main/main.c php_request_shutdown(),
# Zend/zend_execute_API.c shutdown_executor().
COMPILE_FATAL_ALLOWANCE=16384

# What an ordinary request is allowed to cost. Not a difference between two
# paths this time but the whole cost, and the whole cost of a request that
# returns everything it took is zero — so this is a rounding allowance, not a
# budget. One leaked default content type is 32 bytes.
SLACK_PER_REQUEST=8

cd "$(dirname "$0")/.."
# Same compose project as run_all.sh, so a check run by hand drives the
# profile of this checkout rather than one shared with other checkouts.
# shellcheck source=../lib/common.sh
source lib/common.sh
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
# Both readers answer nothing rather than fail when the server is down, so that
# the check that took it down is the one to report it.
read_mem() { curl -s "${base}/fixture_memory.php" | sed -n "s/.*$1=\([0-9]*\).*/\1/p" || true; }

# How many times the server has replaced a worker. Read from outside PHP, so
# the reading costs the worker under measurement nothing.
read_recycles() {
	$COMPOSE exec -T oxphp-fibers wget -qO- http://127.0.0.1:9090/metrics 2> /dev/null \
		| sed -n 's/^oxphp_worker_recycles_total \([0-9]*\)$/\1/p' || true
}

# One connection, N rounds of the given requests in order: each is its own
# request, and the fiber they land on is the same one every time with a single
# worker.
fire() {
	local urls=() _
	for _ in $(seq 1 "$N"); do urls+=("$@"); done
	# `-o` binds to one URL only, so the bodies of the rest would land on stdout
	# — which in --jsonl mode is the report stream.
	curl -s "${urls[@]}" > /dev/null || true
}

fail=0
clean="${base}/fixture_fatal.php"

# Without the count, a replaced worker and a kept one read the same, which is
# the blindness the count is here to remove.
if [ -z "$(read_recycles)" ]; then
	bad "the worker replacement count can be read" "no oxphp_worker_recycles_total on the internal listener"
	$COMPOSE down -v > /dev/null 2>&1
	exit 1
fi

# Fires N rounds of the given requests and sets cost to what one round left on
# the worker, or to nothing when the worker was replaced along the way. A server
# that stops answering ends the run there, as a failure of the named check.
measure() {
	local name="$1" recycles_before used_before used_after
	shift
	recycles_before="$(read_recycles)"
	used_before="$(read_mem used)"
	fire "$@"
	used_after="$(read_mem used)"
	if [ -z "$used_before" ] || [ -z "$used_after" ]; then
		bad "$name" "the server stopped answering during the run"
		$COMPOSE down -v > /dev/null 2>&1
		exit 1
	fi
	cost=$(( (used_after - used_before) / N ))
	replaced=""
	if [ "$(read_recycles)" != "$recycles_before" ]; then
		replaced=1
	fi
}

# What a fatal costs on top of the same request without one, checked against an
# allowance and left in fatal_cost. Each round is the fatal and the clean
# request after it, so the control is paid twice per round.
check_fatal() {
	local name="$1" allowance="$2" url="$3"
	# One round first: what the request declares on its first run stays with
	# the worker on purpose and is not what a fatal costs.
	curl -s -o /dev/null "$url" -o /dev/null "$clean" || true
	measure "$name" "$url" "$clean"
	fatal_cost=$(( cost - 2 * clean_cost ))
	say "  ${name}: ${fatal_cost} B per fatal"
	if [ -n "$replaced" ]; then
		bad "$name" "the worker was replaced during the run, so its readings are not one worker's"
	elif [ "$fatal_cost" -le "$allowance" ]; then
		ok "$name"
	else
		bad "$name" "${fatal_cost} B per fatal over ${N}, allowed ${allowance} B"
	fi
}

# The control: the same request without the fatal.
curl -s -o /dev/null "$clean"
measure "the control ran on one worker" "$clean"
clean_cost="$cost"
if [ -n "$replaced" ]; then
	bad "the control ran on one worker" "the worker was replaced during ${N} requests that all end normally"
fi
say "per request over ${N}: clean ${clean_cost} B"

real_before="$(read_mem real)"
recycles_before="$(read_recycles)"
check_fatal "a fatal costs a worker no more than the same request without one" \
	"$ALLOWED_PER_FATAL" "${clean}?fatal=1"
real_after="$(read_mem real)"
if [ "$(read_recycles)" != "$recycles_before" ]; then
	bad "${N} fatals leave the worker's allocator where it was" \
		"the worker was replaced during the run, so its readings are not one worker's"
elif [ "$real_after" -le "$real_before" ]; then
	ok "${N} fatals leave the worker's allocator where it was"
else
	bad "${N} fatals leave the worker's allocator where it was" "grew from ${real_before} to ${real_after} bytes"
fi

check_fatal "a compile error in eval'd code gives back the syntax tree it was building" \
	"$COMPILE_FATAL_ALLOWANCE" "${clean}?compile=1"
check_fatal "a compile error in an included file gives back the syntax tree it was building" \
	"$COMPILE_FATAL_ALLOWANCE" "${clean}?compile=include"

# A fatal that stops values in flight, each about a kilobyte that the frames it
# interrupts were about to give back, against the same fatal from the same file
# with nothing in flight. See fixture_fatal_temporaries.php.
temporaries="${base}/fixture_fatal_temporaries.php"
check_fatal "a fatal with nothing in flight costs no more than its message" \
	"$ALLOWED_PER_FATAL" "${temporaries}?form=bare"
bare_cost="$fatal_cost"
for form in foreach top operand icall pending trampoline rope rope_init rope_end; do
	check_fatal "a fatal gives back what its frames held in flight: ${form}" \
		"$bare_cost" "${temporaries}?form=${form}"
done

# An ordinary request that leaves the Content-Type to the engine. Not a
# difference between two paths: the whole cost.
curl -s -o /dev/null "${base}/fixture_no_content_type.php"
measure "a request that sends no Content-Type of its own costs the worker nothing" \
	"${base}/fixture_no_content_type.php"
if [ -n "$replaced" ]; then
	bad "a request that sends no Content-Type of its own costs the worker nothing" \
		"the worker was replaced during the run, so its readings are not one worker's"
elif [ "$cost" -le "$SLACK_PER_REQUEST" ]; then
	ok "a request that sends no Content-Type of its own costs the worker nothing"
else
	bad "a request that sends no Content-Type of its own costs the worker nothing" \
		"${cost} B per request over ${N}"
fi

$COMPOSE down -v > /dev/null 2>&1

exit "$fail"
