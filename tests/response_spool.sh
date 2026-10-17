#!/usr/bin/env bash
#
# Integration test for the memory bound on buffered PHP response bodies.
#
# Output past RESPONSE_BUFFER_BYTES goes to an unlinked temporary file, and the
# body is sent from that file once the script ends, so a large download costs
# disk rather than memory while a slow client reads it.
#
# Scenario A1 (bound): ten clients read a 500 MiB readfile() at 1 MiB/s from a
# container capped at 6 GiB. The container must survive, oxphp's anonymous
# memory must stay far below one body, and each body must be held by a spool
# file.
#
# Scenario A2 (cap): with the default RESPONSE_SPOOL_MAX_BYTES (1 GiB) two such
# downloads fit and the third is answered 503 with Retry-After. Once the first
# two are gone their space is free again.
#
# Scenario A3 (the buffered rest): twenty slow clients read a body written in
# small pieces, whose last part is still in the in-memory buffer when the
# script ends. That part goes to the spool file too, so oxphp's anonymous
# memory does not grow with the clients still reading.
#
# Scenario B (integrity): spooled bodies arrive byte for byte with the right
# Content-Length, through one large write and through many small ones; HEAD
# is still answered; a small body is unaffected.
#
# Scenario C (early send and streams): a spooled body is delivered whole at
# oxphp_finish_request(), and a spooled response that turns into a stream
# sends what was spooled first, then streams a large write without spooling
# it.
#
# Scenario D (worker mode): two requests multiplexed on one worker thread,
# each writing past the threshold on both sides of a fiber park, each get
# their own body.
#
# Scenario E (unwritable spool directory): startup warns, a body past the
# threshold is answered 503, and a small one served next on the same worker
# thread is not refused with it.
#
# Scenario F (async tasks): output an oxphp_async() task writes has no client
# and never reaches a spool file.
#
# Scenario G (empty TMPDIR): an empty TMPDIR counts as unset, as it does for
# PHP, so the spool files go to /tmp rather than to the working directory.
#
# NOT wired into run_all.sh or CI (like tests/graceful_drain.sh) — it needs a
# 6 GiB container and several GiB of scratch disk. Run it manually after
# touching the response buffering path.
#
# Usage: tests/response_spool.sh [IMAGE_REF]   (default: oxphp-oxphp:latest)
set -u

IMAGE="${1:-oxphp-oxphp:latest}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FIX="$ROOT/tests/fixtures/spool"
# Container names carry the PID so two runs on one Docker host do not remove
# each other's containers.
SPOOL_A1="spool_a1_$$"
SPOOL_A2="spool_a2_$$"
SPOOL_A3="spool_a3_$$"
SPOOL_B="spool_b_$$"
SPOOL_D="spool_d_$$"
SPOOL_E="spool_e_$$"
SPOOL_F="spool_f_$$"
SPOOL_G="spool_g_$$"
PASS=0
FAIL=0

ok()   { printf '  \033[32mPASS\033[0m %s\n' "$1"; PASS=$((PASS + 1)); }
bad()  { printf '  \033[31mFAIL\033[0m %s\n' "$1"; FAIL=$((FAIL + 1)); }

cleanup() {
	docker rm -f "$SPOOL_A1" "$SPOOL_A2" "$SPOOL_A3" "$SPOOL_B" "$SPOOL_D" "$SPOOL_E" "$SPOOL_F" "$SPOOL_G" >/dev/null 2>&1
}
trap cleanup EXIT

start_container() {
	# start_container <name> [extra docker run args...]
	#
	# The bodies are generated inside the container, in /tmp, which is also
	# where the server spools by default. QUEUE_WAIT_TIMEOUT_MS is raised so
	# that requests queued behind four busy workers are not refused 529 before
	# a worker reaches them.
	local name="$1"
	shift
	docker run -d --name "$name" \
		--memory=6g --memory-swap=6g \
		-e DOCUMENT_ROOT=/var/www/html \
		-e COMPRESSION_ENCODINGS=off \
		-e PHP_WORKERS=4 \
		-e QUEUE_WAIT_TIMEOUT_MS=30000 \
		-e LOG_LEVEL=warn \
		-v "$FIX:/var/www/html:ro" \
		"$@" \
		--entrypoint sh "$IMAGE" -c \
		'head -c 524288000 /dev/urandom > /tmp/big.bin && head -c 3145728 /dev/urandom > /tmp/mid.bin && exec oxphp' \
		>/dev/null || return 1
	for _ in $(seq 1 60); do
		docker exec "$name" curl -fsS http://127.0.0.1/small.php >/dev/null 2>&1 && return 0
		sleep 1
	done
	return 1
}

in_c() { docker exec "$1" sh -c "$2"; }

# Anonymous memory only. readfile() maps the file it sends, and its pages count
# in VmRSS while PHP copies them out; they are the file's page cache, not
# memory the server holds for the response.
anon_kib() { docker exec "$1" awk '/^RssAnon/{print $2}' /proc/1/status 2>/dev/null; }

spool_fds() {
	docker exec "$1" sh -c 'ls -l /proc/1/fd 2>/dev/null | grep -c "oxphp-response-.*(deleted)"'
}

# ── A1: the bound ─────────────────────────────────────────────────
echo "== A1: 10 slow clients on a 500 MiB readfile(), spool cap raised to 8 GiB"
if start_container "$SPOOL_A1" -e RESPONSE_SPOOL_MAX_BYTES=8589934592; then
	BASE=$(anon_kib "$SPOOL_A1")
	docker exec -d "$SPOOL_A1" sh -c 'for i in $(seq 10); do curl -s -o /dev/null --limit-rate 1M http://127.0.0.1/dl.php & done; wait'
	MAX=0
	FDS=0
	for i in $(seq 1 20); do
		sleep 0.5
		R=$(anon_kib "$SPOOL_A1")
		[ -n "$R" ] && [ "$R" -gt "$MAX" ] && MAX=$R
		[ "$i" = 12 ] && FDS=$(spool_fds "$SPOOL_A1")
	done
	STATE=$(docker inspect -f '{{.State.Running}} {{.State.OOMKilled}}' "$SPOOL_A1")
	echo "  base ${BASE} KiB, max ${MAX} KiB over 10 s, spool fds ${FDS}, running/OOMKilled: ${STATE}"
	[ "$STATE" = "true false" ] && ok "A1: container alive, not OOMKilled" || bad "A1: container state ${STATE}"
	[ "$MAX" -gt 0 ] && [ "$MAX" -lt 262144 ] \
		&& ok "A1: oxphp RssAnon peaked at $((MAX / 1024)) MiB (< 256 MiB)" \
		|| bad "A1: oxphp RssAnon peaked at $((MAX / 1024)) MiB"
	[ "$FDS" = "10" ] && ok "A1: each of the 10 bodies is held by an unlinked spool file" \
		|| bad "A1: ${FDS} spool files open, expected 10"
else
	bad "A1: container failed to start"
fi
docker rm -f "$SPOOL_A1" >/dev/null 2>&1

# ── A2: the cap ───────────────────────────────────────────────────
echo "== A2: default spool cap (1 GiB) against three 500 MiB downloads"
if start_container "$SPOOL_A2"; then
	docker exec -d "$SPOOL_A2" sh -c 'for i in 1 2; do curl -s -o /dev/null -w "%{http_code}\n" --max-time 6 --limit-rate 1M http://127.0.0.1/dl.php > /tmp/a2_$i.txt & done; wait'
	sleep 3
	THIRD=$(in_c "$SPOOL_A2" 'curl -s -D - -o /tmp/a2_3.body http://127.0.0.1/dl.php')
	sleep 5
	FIRST_TWO=$(in_c "$SPOOL_A2" 'cat /tmp/a2_1.txt /tmp/a2_2.txt' | tr '\n' ' ')
	THIRD_STATUS=$(printf '%s' "$THIRD" | head -1 | awk '{print $2}')
	echo "  first two: ${FIRST_TWO}| third: ${THIRD_STATUS}"
	[ "$FIRST_TWO" = "200 200 " ] && ok "A2: two downloads within the cap are served" \
		|| bad "A2: first two answered ${FIRST_TWO}"
	[ "$THIRD_STATUS" = "503" ] && ok "A2: the download past the cap is answered 503" \
		|| bad "A2: the download past the cap is answered ${THIRD_STATUS}"
	printf '%s' "$THIRD" | grep -qi '^retry-after:' && ok "A2: the 503 carries Retry-After" \
		|| bad "A2: the 503 has no Retry-After"
	printf '%s' "$THIRD" | grep -qi '^content-length: 524288000' \
		&& bad "A2: the 503 still carries the script's Content-Length" \
		|| ok "A2: the 503 drops the script's Content-Length"
	docker logs "$SPOOL_A2" 2>&1 | grep -q 'RESPONSE_SPOOL_MAX_BYTES' \
		&& ok "A2: the refusal is logged naming RESPONSE_SPOOL_MAX_BYTES" \
		|| bad "A2: no refusal in the log"
	AFTER=$(in_c "$SPOOL_A2" 'curl -s -o /dev/null -w "%{http_code}" --max-time 3 --limit-rate 1M http://127.0.0.1/dl.php')
	[ "$AFTER" = "200" ] && ok "A2: space is free again once the first downloads are gone" \
		|| bad "A2: a download after the first two ended is answered ${AFTER}"
else
	bad "A2: container failed to start"
fi
docker rm -f "$SPOOL_A2" >/dev/null 2>&1

# ── A3: the buffered rest ─────────────────────────────────────────
echo "== A3: 20 slow clients on 48 MiB written in 64 KiB pieces, RESPONSE_BUFFER_BYTES at 32 MiB"
if start_container "$SPOOL_A3" -e RESPONSE_BUFFER_BYTES=33554432 -e RESPONSE_SPOOL_MAX_BYTES=8589934592; then
	BASE=$(anon_kib "$SPOOL_A3")
	docker exec -d "$SPOOL_A3" sh -c 'for i in $(seq 20); do curl -s -o /dev/null --limit-rate 1M http://127.0.0.1/tail.php & done; wait'
	# Each request opens its spool file when it crosses the threshold, and
	# writes its last 16 MiB a moment later; the clients take 30 s and more to
	# read the first 32 MiB, so all twenty bodies are still being read below.
	FDS=0
	for _ in $(seq 1 60); do
		FDS=$(spool_fds "$SPOOL_A3")
		[ "$FDS" = "20" ] && break
		sleep 0.5
	done
	sleep 3
	MIN=
	for _ in $(seq 1 4); do
		sleep 0.5
		R=$(anon_kib "$SPOOL_A3")
		[ -n "$R" ] && { [ -z "$MIN" ] || [ "$R" -lt "$MIN" ]; } && MIN=$R
	done
	echo "  base ${BASE} KiB, spool fds ${FDS}, RssAnon ${MIN:-?} KiB with every script done"
	[ "$FDS" = "20" ] && ok "A3: all twenty bodies are held by spool files" \
		|| bad "A3: ${FDS} spool files open, expected 20"
	[ -n "$MIN" ] && [ $((MIN - BASE)) -lt 131072 ] \
		&& ok "A3: oxphp RssAnon grew by $(((MIN - BASE) / 1024)) MiB (< 128 MiB)" \
		|| bad "A3: oxphp RssAnon grew by $(((${MIN:-0} - BASE) / 1024)) MiB"
else
	bad "A3: container failed to start"
fi
docker rm -f "$SPOOL_A3" >/dev/null 2>&1

# ── B, C: integrity, early send, streams ──────────────────────────
echo "== B/C: integrity of spooled bodies"
if start_container "$SPOOL_B"; then
	BIG_SHA=$(in_c "$SPOOL_B" 'sha256sum /tmp/big.bin' | awk '{print $1}')
	MID_SHA=$(in_c "$SPOOL_B" 'sha256sum /tmp/mid.bin' | awk '{print $1}')
	GOT=$(in_c "$SPOOL_B" 'curl -s http://127.0.0.1/dl.php | sha256sum' | awk '{print $1}')
	[ "$GOT" = "$BIG_SHA" ] && ok "B: readfile() of 500 MiB arrives intact" || bad "B: readfile() body differs"
	GOT=$(in_c "$SPOOL_B" 'curl -s http://127.0.0.1/mid.php | sha256sum' | awk '{print $1}')
	[ "$GOT" = "$MID_SHA" ] && ok "B: 3 MiB in 8 KiB writes arrives intact" || bad "B: 8 KiB-write body differs"
	CL=$(in_c "$SPOOL_B" 'curl -s -D - -o /dev/null http://127.0.0.1/mid.php' | tr -d '\r' | awk -F': ' 'tolower($1)=="content-length"{print $2}')
	[ "$CL" = "3145728" ] && ok "B: a spooled body without a script Content-Length gets the exact one" \
		|| bad "B: Content-Length is '${CL}'"
	# PHP writes no body for HEAD (SG(request_info).headers_only), so nothing
	# reaches the spool; the answer only has to stay what it was.
	HEAD=$(in_c "$SPOOL_B" 'curl -s -I -o /dev/null -w "%{http_code}" http://127.0.0.1/mid.php')
	[ "$HEAD" = "200" ] && ok "B: HEAD is answered 200" || bad "B: HEAD answered ${HEAD}"
	SMALL=$(in_c "$SPOOL_B" 'curl -s -D - http://127.0.0.1/small.php' | tr -d '\r')
	printf '%s' "$SMALL" | grep -qi '^content-length: 5$' && printf '%s' "$SMALL" | tail -1 | grep -q '^hello$' \
		&& ok "B: a small body is unaffected" || bad "B: small body: $(printf '%s' "$SMALL" | tr '\n' '|')"

	T0=$(date +%s)
	GOT=$(in_c "$SPOOL_B" 'curl -s http://127.0.0.1/finish.php | sha256sum' | awk '{print $1}')
	T1=$(date +%s)
	[ "$GOT" = "$MID_SHA" ] && ok "C: a spooled body is delivered whole at oxphp_finish_request()" \
		|| bad "C: finish_request body differs"
	[ $((T1 - T0)) -lt 3 ] && ok "C: ...before the script ends ($((T1 - T0))s)" \
		|| bad "C: finish_request body took $((T1 - T0))s"
	WANT=$(in_c "$SPOOL_B" '{ cat /tmp/mid.bin /tmp/mid.bin; printf TAIL; } | sha256sum' | awk '{print $1}')
	GOT=$(in_c "$SPOOL_B" 'curl -s http://127.0.0.1/stream.php | sha256sum' | awk '{print $1}')
	[ "$GOT" = "$WANT" ] && ok "C: a spooled response turned stream sends the spooled bytes first, then the stream" \
		|| bad "C: stream body differs"
else
	bad "B: container failed to start"
fi
docker rm -f "$SPOOL_B" >/dev/null 2>&1

# ── D: worker mode, two fibers on one thread ──────────────────────
echo "== D: worker mode, two requests spooling across a fiber park"
if start_container "$SPOOL_D" -e WORKER_MODE_ENABLED=true -e ENTRY_FILE=/var/www/html/worker.php -e PHP_WORKERS=1 -e LOG_LEVEL=info; then
	in_c "$SPOOL_D" 'curl -s "http://127.0.0.1/x?id=66" > /tmp/d66 & curl -s "http://127.0.0.1/x?id=67" > /tmp/d67 & wait'
	A=$(in_c "$SPOOL_D" 'wc -c < /tmp/d66; tr -d B < /tmp/d66 | wc -c' | tr '\n' ' ')
	B=$(in_c "$SPOOL_D" 'wc -c < /tmp/d67; tr -d C < /tmp/d67 | wc -c' | tr '\n' ' ')
	# The premise: both requests were inside the handler at once, so the
	# second started before the first ended. The readiness probe runs the
	# handler too, with the default id, so only these two ids count.
	ORDER=$(docker logs "$SPOOL_D" 2>&1 | grep -oE 'spool-worker: [a-z]+ (66|67)' | awk '{print $2}' | tr '\n' ' ')
	echo "  66: ${A}| 67: ${B}| order: ${ORDER}"
	[ "$ORDER" = "start start end end " ] && ok "D: the two requests were multiplexed" \
		|| bad "D: the requests did not overlap (${ORDER})"
	[ "$A" = "4194304 0 " ] && [ "$B" = "4194304 0 " ] && ok "D: each request got its own 4 MiB body" \
		|| bad "D: bodies mixed or short"
else
	bad "D: container failed to start"
fi
docker rm -f "$SPOOL_D" >/dev/null 2>&1

# ── E: unwritable spool directory ─────────────────────────────────
echo "== E: TMPDIR not writable"
# One worker thread, so the small request runs where the refused one did.
if start_container "$SPOOL_E" -e TMPDIR=/nonexistent -e PHP_WORKERS=1; then
	docker logs "$SPOOL_E" 2>&1 | grep -q 'not writable' && ok "E: startup warns about the spool directory" \
		|| bad "E: no startup warning"
	S=$(in_c "$SPOOL_E" 'curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1/mid.php')
	[ "$S" = "503" ] && ok "E: a body past the threshold is answered 503" || bad "E: large body answered ${S}"
	S=$(in_c "$SPOOL_E" 'curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1/small.php')
	[ "$S" = "200" ] && ok "E: a small body is served" || bad "E: small body answered ${S}"
else
	bad "E: container failed to start"
fi
docker rm -f "$SPOOL_E" >/dev/null 2>&1

# ── F: async task output ──────────────────────────────────────────
echo "== F: an async task writing past the threshold"
if start_container "$SPOOL_F" -e ASYNC_WORKERS=1; then
	R=$(in_c "$SPOOL_F" 'curl -s http://127.0.0.1/async.php')
	# The task's thread keeps its output until the pool is idle; a spool file
	# would stay open on it for good, so count right after the task returned.
	FDS=$(spool_fds "$SPOOL_F")
	[ "$R" = "done" ] && ok "F: the task ran" || bad "F: the task answered '${R}'"
	[ "$FDS" = "0" ] && ok "F: no spool file for the task's output" || bad "F: ${FDS} spool files open"
else
	bad "F: container failed to start"
fi
docker rm -f "$SPOOL_F" >/dev/null 2>&1

# ── G: empty TMPDIR ───────────────────────────────────────────────
echo "== G: TMPDIR set but empty"
# Taken as a path, an empty TMPDIR is the working directory: / in the image,
# which the server's user cannot write to.
if start_container "$SPOOL_G" -e TMPDIR=; then
	docker logs "$SPOOL_G" 2>&1 | grep -q 'not writable' && bad "G: startup warns about the spool directory" \
		|| ok "G: no startup warning"
	# A body larger than the socket buffers, so its spool file is still being
	# read when we look: the file is closed once its last byte has been read.
	docker exec -d "$SPOOL_G" sh -c 'curl -s -o /dev/null --limit-rate 1M http://127.0.0.1/dl.php'
	sleep 3
	WHERE=$(in_c "$SPOOL_G" 'ls -l /proc/1/fd' | grep -o '[^ ]*oxphp-response-[^ ]*' | head -1)
	case "$WHERE" in
		/tmp/oxphp-response-*) ok "G: the spool file is in /tmp" ;;
		*) bad "G: spool file at '${WHERE}'" ;;
	esac
	S=$(in_c "$SPOOL_G" 'curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1/mid.php')
	[ "$S" = "200" ] && ok "G: a body past the threshold is served" || bad "G: large body answered ${S}"
else
	bad "G: container failed to start"
fi

echo
echo "== result: $PASS passed, $FAIL failed =="
[ "$FAIL" -eq 0 ]
