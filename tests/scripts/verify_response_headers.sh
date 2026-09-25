#!/usr/bin/env bash
# Verify that a worker's response carries exactly what the request put in it.
#
# A worker serves many requests inside one PHP request lifetime, so the points
# where the engine would normally finish a response — flushing what is left of
# the output buffers, sending the headers — do not come around by themselves.
# Where the server puts them decides what a client gets, and getting it wrong is
# invisible to the request itself: it shows up in the response, or in the next
# request's response, which nothing inside a request can look at.
#
# Three things, all measured from outside:
#
#   1. the first response a worker sends carries one Content-Type, not two. The
#      engine can be made to emit its default while a request is starting up,
#      and that copy has to be cleared along with the rest of the header state,
#      or it goes out on top of the one the request itself produces.
#   2. a response that writes nothing still carries a Content-Type, the way it
#      does under every other SAPI.
#   3. an output buffer a request leaves open belongs to that request's own
#      response. Flushed at the start of the next request instead, its content
#      is delivered to a different client than the one that asked for it.
#
# Usage: verify_response_headers.sh [--jsonl]
#   --jsonl  emit one result object per check on stdout instead of a human
#            report, for run_all.sh to fold into its report
set -euo pipefail

JSONL=""
for arg in "$@"; do
	case "$arg" in
		--jsonl) JSONL=1 ;;
		*) echo "Unknown argument: $arg" >&2; exit 1 ;;
	esac
done

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

# The first response a worker sends is itself under test, so the profile is
# brought up here rather than reused: a worker that has already served a request
# is a different subject. The checks that follow the first one run against that
# same worker, in order.
fresh_worker() {
	$COMPOSE down -v > /dev/null 2>&1 || true
	$COMPOSE up -d --wait >&2
	port="$($COMPOSE port oxphp-fibers 80 | head -1 | cut -d: -f2)"
	base="http://127.0.0.1:${port}/tests/fibers"
}

# Every request keeps what curl said about it under its own name. A count of
# zero has two readings — a response without the header, or no response at all
# — and only curl's own error tells them apart, so a failing check reports the
# error of its own request next to what it measured.
errdir="$(mktemp -d)"
trap 'rm -rf "$errdir"' EXIT
fetch() { local name="$1"; shift; curl -sS "$@" 2>"${errdir}/${name}"; }
curl_error() { if [ -s "${errdir}/$1" ]; then printf ' (%s)' "$(head -1 "${errdir}/$1")"; fi; }

count_content_type() { fetch "$1" -D- -o /dev/null "$2" | tr -d '\r' | grep -ci '^content-type:' || true; }

# What the worker looked like when a request failed at the transport level,
# taken before the teardown that would otherwise destroy it: a container that
# has exited says the server went away; with a live one, curl's error says
# whether the connection was refused or dropped.
diagnose() {
	echo "A request to ${base} failed. Requests in the order they were sent:"
	local name err
	for name in first second empty buffered next; do
		err="$(head -1 "${errdir}/${name}" 2>/dev/null || true)"
		printf '  %-8s %s\n' "$name" "${err:-(no curl error)}"
	done
	$COMPOSE ps -a
	local id
	id="$($COMPOSE ps -aq oxphp-fibers)"
	if [ -n "$id" ]; then docker inspect -f '{{json .State}}' "$id"; fi
	$COMPOSE logs --tail=50 oxphp-fibers
}

fail=0

# 1. The worker's very first response.
fresh_worker
first="$(count_content_type first "${base}/fixture_no_content_type.php")"
second="$(count_content_type second "${base}/fixture_no_content_type.php")"
if [ "$first" = "1" ]; then
	ok "the worker's first response carries one Content-Type"
else
	bad "the worker's first response carries one Content-Type" "carried ${first}$(curl_error first)"
fi
if [ "$second" = "1" ]; then
	ok "later responses carry one Content-Type"
else
	bad "later responses carry one Content-Type" "carried ${second}$(curl_error second)"
fi

# 2. A response that writes nothing.
empty="$(count_content_type empty "${base}/fixture_header_no_body.php")"
if [ "$empty" = "1" ]; then
	ok "a response with no body still carries a Content-Type"
else
	bad "a response with no body still carries a Content-Type" "carried ${empty}$(curl_error empty)"
fi

# 3. An output buffer left open.
buffered="$(fetch buffered "${base}/fixture_unclosed_buffer.php" || true)"
next="$(fetch next "${base}/fixture_no_content_type.php" || true)"
if grep -qs . "$errdir"/*; then diagnose >&2 || true; fi
$COMPOSE down -v > /dev/null 2>&1

case "$buffered" in
	*buffered-by-its-own-request*)
		ok "an output buffer left open is delivered to the request that opened it" ;;
	*)
		bad "an output buffer left open is delivered to the request that opened it" \
			"its own response was [${buffered}]$(curl_error buffered)" ;;
esac

# The next response has to be its own fixture's output and nothing else: an
# empty one carries no foreign buffer either, but only because nothing arrived.
case "$next" in
	ok)
		ok "the next response does not carry the previous request's buffer" ;;
	*)
		bad "the next response does not carry the previous request's buffer" \
			"it was [${next}]$(curl_error next)" ;;
esac

exit "$fail"
