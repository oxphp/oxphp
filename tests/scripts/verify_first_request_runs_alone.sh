#!/usr/bin/env bash
# Verify that, with runtime hooks installed, the first request a worker takes
# runs alone while it waits, for a bounded time and only that once.
#
# A framework's container is built by the first request that needs it. Under
# runtime hooks a socket read inside that build parks the request, and a request
# admitted beside it finds the build half done. So the worker admits nothing
# beside its first request until that request ends or a quarter of a second
# after it first waits, which is short of what the queue's default wait budget
# gives the requests held back.
#
# Each scenario needs a worker that has served nothing, which no request can
# arrange for itself, so this drives the profile from outside. What is compared
# is when each request started and what it found, as measured by the server:
# which of two responses comes back first says less than which request was let in
# first.
#
# Usage: verify_first_request_runs_alone.sh [--jsonl]
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
COMPOSE="docker compose -f compose.yml -f compose.hooks.yml"

override="$(mktemp -t oxphp-first-alone.XXXXXX)"
printf 'services:\n  oxphp-hooks:\n    environment:\n      RUNTIME_HOOKS: ""\n' > "$override"

emit() {
	python3 -c 'import json,sys; print(json.dumps({"test": sys.argv[1], "group": "hooks", "pass": sys.argv[2] == "1", "assertions": [], "error": sys.argv[3], "meta": {}, "profile": "hooks"}, ensure_ascii=False))' "$1" "$2" "$3"
}
ok() {
	if [ -n "$JSONL" ]; then emit "$1" 1 ""; else printf '  \033[32mPASS\033[0m %s\n' "$1"; fi
}
bad() {
	if [ -n "$JSONL" ]; then emit "$1" 0 "$2"; else printf '  \033[31mFAIL\033[0m %s: %s\n' "$1" "$2"; fi
	fail=1
}

# Each scenario brings its own stack up under set -e, and the checks of the ones
# before it are already reported: without a line saying so, run_all.sh would take
# those for the whole check.
finished=""
on_exit() {
	rm -f "$override"
	[ -n "$finished" ] || bad "the check ran through every scenario" "a compose step failed partway"
}
trap on_exit EXIT

fail=0
fixture="/tests/hooks/fixture_lazy_build.php"

# Starts one request, and a second one a moment later, on two threads of one
# process: forking a curl for each would put tens of milliseconds of jitter into
# an offset that is itself only forty. Reports both answers and whether the second
# really went out while the first was still in flight, without which the scenario
# has exercised nothing.
read -r -d '' driver <<'PY' || true
import http.client, json, sys, threading, time

port, first, second, offset_ms = int(sys.argv[1]), sys.argv[2], sys.argv[3], int(sys.argv[4])
results = {}

def get(name, path):
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=15)
    conn.request("GET", path)
    response = conn.getresponse()
    body = response.read().decode()
    try:
        parsed = json.loads(body)
    except ValueError:
        parsed = {"unparsed": body}
    results[name] = {"status": response.status, "body": parsed, "done": time.monotonic()}

threads = [threading.Thread(target=get, args=("first", first)), threading.Thread(target=get, args=("second", second))]
threads[0].start()
time.sleep(offset_ms / 1000)
sent = time.monotonic()
threads[1].start()
for thread in threads:
    thread.join()
results["sent_while_first_in_flight"] = sent < results["first"]["done"]
print(json.dumps(results))
PY

field() {
	python3 -c 'import json,sys
try:
    v = json.loads(sys.stdin.read())
    for k in sys.argv[1].split("."):
        v = v[k]
    print(json.dumps(v) if not isinstance(v, str) else v)
except Exception:
    print("")' "$1"
}

# A worker that has served nothing, and the port it answers on.
fresh_worker() {
	$1 up -d --wait --force-recreate >&2
	port="$($1 port oxphp-hooks 80 | head -1 | cut -d: -f2)"
	base="http://127.0.0.1:${port}"
}

hooks_installed() {
	{ $1 exec -T oxphp-hooks wget -qO- http://127.0.0.1:9090/config || true; } | field runtime_hooks
}

# The premises every overlap below rests on.
check_overlap() {
	local name="$1" pair="$2"
	if [ "$(printf '%s' "$pair" | field first.status)" = "200" ] && [ "$(printf '%s' "$pair" | field second.status)" = "200" ]; then
		ok "$name: premise: both requests were answered"
	else
		bad "$name: premise: both requests were answered" "$pair"
	fi
	if [ "$(printf '%s' "$pair" | field sent_while_first_in_flight)" = "true" ]; then
		ok "$name: premise: the second request went out while the first was in flight"
	else
		bad "$name: premise: the second request went out while the first was in flight" "$pair"
	fi
	# Taken the other way round, the probe would be the worker's first request and
	# every assertion below would be about the wrong one.
	local parked_at started_at
	parked_at="$(printf '%s' "$pair" | field first.body.parked_at)"
	started_at="$(printf '%s' "$pair" | field second.body.started_at)"
	if [ -n "$parked_at" ] && [ -n "$started_at" ] && [ "$parked_at" -lt "$started_at" ]; then
		ok "$name: premise: the build began before the probe was let in"
	else
		bad "$name: premise: the build began before the probe was let in" "$pair"
	fi
}

# Nanoseconds from one server-side time to a later one, printed in milliseconds.
span_ms() {
	python3 -c 'import sys; print(int((int(sys.argv[2]) - int(sys.argv[1])) / 1000000))' "$1" "$2"
}

# ── A wait shorter than the window: nothing is let in until the build is done ──
fresh_worker "$COMPOSE"
hooks="$(hooks_installed "$COMPOSE")"
if [ -n "$hooks" ] && [ "$hooks" != "[]" ]; then
	ok "premise: the worker has runtime hooks installed"
else
	bad "premise: the worker has runtime hooks installed" "/config says: ${hooks:-nothing}"
fi

pair="$(python3 -c "$driver" "$port" "${fixture}?role=build&park_ms=150" "${fixture}?role=probe" 40 || true)"
check_overlap "short wait" "$pair"
started="$(printf '%s' "$pair" | field second.body.started_at)"
ended="$(printf '%s' "$pair" | field first.body.ended_at)"
if [ -n "$started" ] && [ -n "$ended" ] && [ "$started" -gt "$ended" ]; then
	ok "a request arriving during the first request's wait is let in after that request ends"
else
	bad "a request arriving during the first request's wait is let in after that request ends" "$pair"
fi
if [ "$(printf '%s' "$pair" | field second.body.seen.building)" = "false" ]; then
	ok "that request finds the first request's build finished"
else
	bad "that request finds the first request's build finished" "$pair"
fi

# ── A wait longer than the window: held for the window, and not past it ────────
fresh_worker "$COMPOSE"
pair="$(python3 -c "$driver" "$port" "${fixture}?role=build&park_ms=2000" "${fixture}?role=probe" 40 || true)"
check_overlap "long wait" "$pair"
started="$(printf '%s' "$pair" | field second.body.started_at)"
parked="$(printf '%s' "$pair" | field first.body.parked_at)"
ended="$(printf '%s' "$pair" | field first.body.ended_at)"
if [ -n "$started" ] && [ -n "$parked" ] && [ -n "$ended" ]; then
	held="$(span_ms "$parked" "$started")"
	if [ "$held" -ge 200 ]; then
		ok "while the first request waits, the next one is held back for the window"
	else
		bad "while the first request waits, the next one is held back for the window" "let in ${held} ms after the first began to wait"
	fi
	if [ "$held" -lt 600 ]; then
		ok "the window closes about when it should"
	else
		bad "the window closes about when it should" "let in ${held} ms after the first began to wait"
	fi
	if [ "$started" -lt "$ended" ]; then
		ok "a first request that waits past the window does not keep the worker to itself"
	else
		bad "a first request that waits past the window does not keep the worker to itself" "let in ${held} ms after the first began to wait, which ended after $(span_ms "$parked" "$ended") ms"
	fi
else
	bad "while the first request waits, the next one is held back for the window" "$pair"
	bad "the window closes about when it should" "$pair"
	bad "a first request that waits past the window does not keep the worker to itself" "$pair"
fi

# ── Work before the first wait: the window opens at the wait, not at the start ─
fresh_worker "$COMPOSE"
pair="$(python3 -c "$driver" "$port" "${fixture}?role=build&busy_ms=400&park_ms=1000" "${fixture}?role=probe" 40 || true)"
check_overlap "busy first" "$pair"
started="$(printf '%s' "$pair" | field second.body.started_at)"
parked="$(printf '%s' "$pair" | field first.body.parked_at)"
if [ -n "$started" ] && [ -n "$parked" ]; then
	held="$(span_ms "$parked" "$started")"
	if [ "$held" -ge 200 ]; then
		ok "a first request that works before it waits still gets the whole window"
	else
		bad "a first request that works before it waits still gets the whole window" "let in ${held} ms after the first began to wait"
	fi
else
	bad "a first request that works before it waits still gets the whole window" "$pair"
fi

# ── Long work, then a short wait: not overlapped, with the budget nearly spent ─
# Measured from the first wait rather than capped from the start, the window
# still covers a build that does most of its work before its only socket read.
fresh_worker "$COMPOSE"
pair="$(python3 -c "$driver" "$port" "${fixture}?role=build&busy_ms=800&park_ms=50" "${fixture}?role=probe" 40 || true)"
check_overlap "busy, short wait" "$pair"
if [ "$(printf '%s' "$pair" | field second.body.seen.building)" = "false" ]; then
	ok "a build that waits briefly after long work is not overlapped"
else
	bad "a build that waits briefly after long work is not overlapped" "$pair"
fi

# ── Only the first: the worker multiplexes as usual from the next request on ───
fresh_worker "$COMPOSE"
opener="$(curl -s --max-time 10 "${base}${fixture}?role=probe")"
if [ -n "$(printf '%s' "$opener" | field started_at)" ]; then
	ok "premise: the worker's first request ended without waiting"
else
	bad "premise: the worker's first request ended without waiting" "answered: ${opener:-nothing}"
fi
pair="$(python3 -c "$driver" "$port" "${fixture}?role=build&park_ms=1000" "${fixture}?role=probe" 40 || true)"
check_overlap "later requests" "$pair"
started="$(printf '%s' "$pair" | field second.body.started_at)"
parked="$(printf '%s' "$pair" | field first.body.parked_at)"
if [ -n "$started" ] && [ -n "$parked" ]; then
	held="$(span_ms "$parked" "$started")"
	# Held back at all, a request starts no sooner than 250 ms after the wait
	# began: the window opens after parked_at is taken.
	if [ "$held" -lt 225 ]; then
		ok "once the first request has ended, the next ones are let in beside a waiting request at once"
	else
		bad "once the first request has ended, the next ones are let in beside a waiting request at once" "let in ${held} ms after the other began to wait"
	fi
else
	bad "once the first request has ended, the next ones are let in beside a waiting request at once" "$pair"
fi

# ── Without runtime hooks a request waits only where it asked to ───────────────
with_override="$COMPOSE -f $override"
fresh_worker "$with_override"
hooks="$(hooks_installed "$with_override")"
if [ "$hooks" = "[]" ]; then
	ok "premise: the worker has no runtime hooks installed"
else
	bad "premise: the worker has no runtime hooks installed" "/config says: ${hooks:-nothing}"
fi
pair="$(python3 -c "$driver" "$port" "${fixture}?role=build&via=sleep&park_ms=200" "${fixture}?role=probe" 40 || true)"
check_overlap "no hooks" "$pair"
started="$(printf '%s' "$pair" | field second.body.started_at)"
ended="$(printf '%s' "$pair" | field first.body.ended_at)"
if [ -n "$started" ] && [ -n "$ended" ] && [ "$started" -lt "$ended" ]; then
	ok "without runtime hooks the next request is let in beside the first one's explicit wait"
else
	bad "without runtime hooks the next request is let in beside the first one's explicit wait" "$pair"
fi

$with_override down -v > /dev/null 2>&1

finished=1
[ -n "$JSONL" ] || [ "$fail" != "0" ] || echo "PASS: a worker's first request runs alone while it waits, for a bounded time and only that once"
exit "$fail"
