#!/usr/bin/env bash
#
# Integration test: a symlink to a script that is repointed after it was first
# served must neither execute nor print a file outside DOCUMENT_ROOT.
#
# Traditional mode, PHP_WORKERS=1. The second request for a path is answered
# from the route cache, which carries the "inside the root" verdict reached for
# the first one and has no expiry; the script is opened later, by the worker.
# Every scenario therefore serves a link once, repoints it from inside the
# container, and serves it again.
#
# PHP keeps its own per-thread realpath cache (`realpath_cache_ttl`, 120 s by
# default) that maps the link to the target it resolved to first, so right
# after a swap the engine still opens the old target. The matrix turns that
# cache off (`realpath_cache_size=0`; a TTL of 0 would not do — it means
# entries never expire) to see the swap at once; one scenario keeps the
# defaults and waits the TTL out.
#
# Two OPcache settings, because OPcache decides whether the engine opens the
# file at all: with `opcache.validate_timestamps=0` a script that is already
# compiled is served from memory without a file open, so a swap goes unseen and
# the scenario would pass on the unfixed build for the wrong reason.
#   off — `opcache.enable=0`: every request opens the script.
#   on  — enabled, `validate_timestamps=1`, `revalidate_freq=0`: a changed
#         mtime recompiles from the path.
#
# Scenarios (per OPcache setting):
#   leaf     link -> inside, then -> an executable script outside the root
#   text     link -> inside, then -> a text file outside (printed verbatim)
#   dir      directory link in the middle of the path repointed outside
#   inside   link -> inside, then -> another file inside (still executes)
#   ident    __FILE__ / __DIR__ / SCRIPT_FILENAME of a linked script
#   cwd      getcwd() of a linked script: the directory of the checked file
#   hits     (on only) a hot script keeps hitting OPcache, no re-compiles
# Plus, with `opcache.validate_timestamps=0` and the script already compiled
# (the engine does not open it then): the swap is still refused, a link
# repointed inside the root runs the new target (one included file, not two), a
# link that was `include`d while it pointed outside does not leave its target
# behind as the script that runs, and scripts reached through links keep
# hitting the cache.
# Plus: the swap after the default realpath cache has expired, a FIFO swapped in for the script (does not hang the worker), a FIFO or a
# directory inside the root (500, logged), and SYMLINK_ALLOW_PATHS (an allow-listed target executes, a sibling does not).
# Plus, on how the open file reaches the engine: a script named *.phar* that is a
# tar, a zip or a compressed phar (the phar extension swaps the handle it is
# given; the descriptor must still be released), a script whose read fails
# with EIO (a failed read must fail the request, not run what was read so far),
# the descriptor the script is read from being in blocking mode, and which name
# the phar extension goes by (the resolved one, not the requested one).
# Plus, with open_basedir set: an entry script outside it is refused as the
# engine refuses any script it cannot open, one inside it runs, a link inside
# the root that resolves outside it is refused, and no include form
# finds an outside file left in OPcache by an entry run (there is none). The
# check is made where the engine makes it, when the script is compiled: a
# relative entry ("open_basedir=.") means the script's own directory, not the
# process's, and an open_basedir that auto_prepend_file narrows applies to the
# script it prepends to, after the prepended file has run, even where OPcache
# holds that script.
# Plus, scripts that cannot be opened: one deleted after OPcache compiled it
# answers 404 (it used to run from the cache), an unreadable one a plain 500.
#
# NOT wired into run_all.sh or CI (like tests/graceful_drain.sh): the suite
# runner cannot repoint a link between two requests. Run manually after
# touching script opening (src/path_guard.rs, the traditional executor).
#
# Usage: tests/symlink_swap_exec.sh [IMAGE_REF]   (default: oxphp-oxphp:latest)
set -u

IMAGE="${1:-oxphp-oxphp:latest}"
# Container names carry the PID so two runs on one Docker host do not remove
# each other's containers.
SUFFIX="$$"
PASS=0
FAIL=0
TMP="$(mktemp -d)"
CONTAINERS=()

ok()  { printf '  \033[32mPASS\033[0m %s\n' "$1"; PASS=$((PASS + 1)); }
bad() { printf '  \033[31mFAIL\033[0m %s\n' "$1"; FAIL=$((FAIL + 1)); }

cleanup() {
	[ "${#CONTAINERS[@]}" -gt 0 ] && docker rm -fv "${CONTAINERS[@]}" >/dev/null 2>&1
	rm -rf "$TMP"
}
trap cleanup EXIT

printf 'opcache.enable=0\nrealpath_cache_size=0\n' > "$TMP/off.ini"
printf 'opcache.enable=1\nopcache.validate_timestamps=1\nopcache.revalidate_freq=0\nopcache.file_update_protection=0\nrealpath_cache_size=0\n' > "$TMP/on.ini"
printf 'opcache.enable=0\n' > "$TMP/default.ini"
printf 'opcache.enable=1\nopcache.validate_timestamps=0\nopcache.file_update_protection=0\nrealpath_cache_size=0\n' > "$TMP/cached.ini"
# open_basedir lists the one directory under test and the readiness probe's own
# file, which lives outside it. display_errors puts the engine's refusal in the
# response body so that its text can be compared.
# revalidate_path decides whether a plain include is looked up in OPcache by its
# name, so both settings are run.
for rp in 0 1; do
	printf 'opcache.enable=1\nopcache.validate_timestamps=0\nopcache.file_update_protection=0\nopcache.revalidate_path=%s\nrealpath_cache_size=0\ndisplay_errors=1\nopen_basedir=/srv/site/a:/srv/site/ready.php\n' "$rp" > "$TMP/obd${rp}.ini"
done

# A relative entry, and an open_basedir that a prepended file narrows.
printf 'opcache.enable=1\nopcache.validate_timestamps=0\nopcache.file_update_protection=0\nrealpath_cache_size=0\ndisplay_errors=1\nopen_basedir=.\n' > "$TMP/obdot.ini"
printf 'opcache.enable=1\nopcache.validate_timestamps=0\nopcache.file_update_protection=0\nrealpath_cache_size=0\ndisplay_errors=1\nopen_basedir=/srv/site\nauto_prepend_file=/srv/site/prepend.php\n' > "$TMP/obdpre.ini"
printf 'opcache.enable=1\nopcache.validate_timestamps=0\nopcache.file_update_protection=0\nrealpath_cache_size=0\ndisplay_errors=1\nopen_basedir=/srv/site/a:/srv/site/prepend.php:/srv/site/ready.php\nauto_prepend_file=/srv/site/prepend.php\n' > "$TMP/obdpre2.ini"

# The trees are built once here and copied into each container before it
# starts: SYMLINK_ALLOW_PATHS entries must exist at startup, and the server
# serves as www-data, so everything has to be world-readable.
SITE="$TMP/fixture/site"
OUT="$TMP/fixture/outside"
mkdir -p "$SITE/real" "$SITE/a" "$SITE/b" "$OUT/dir" "$OUT/allowed"
printf '<?php echo "READY";' > "$SITE/ready.php"
printf '<?php echo "INSIDE_MARKER";' > "$SITE/inside.php"
printf '<?php echo "INSIDE2_MARKER";' > "$SITE/inside2.php"
printf '<?php echo "INSIDE_MARKER";' > "$SITE/real/c.php"
printf '<?php echo __FILE__, "|", $_SERVER["SCRIPT_FILENAME"], "|", __DIR__;' > "$SITE/real/ident.php"
printf '<?php $s = opcache_get_status(false)["opcache_statistics"]; echo $s["hits"], "|", $s["misses"];' > "$SITE/hits.php"
printf '<?php echo "INCL_A|", count(get_included_files());' > "$SITE/incl_a.php"
printf '<?php echo "INCL_B|", count(get_included_files());' > "$SITE/incl_b.php"
printf '<?php echo getcwd(), "|", __DIR__;' > "$SITE/real/cwd.php"
printf '<?php echo getcwd(), "|", __DIR__;' > "$SITE/cwd_root.php"
printf '<?php include "/srv/site/poison.php";' > "$SITE/include_driver.php"
printf '<?php var_export(file_exists("/srv/site/rpcdir/c.php"));' > "$SITE/rpc_driver.php"
printf '<?php echo "OBD_INSIDE";' > "$SITE/a/inside.php"
printf '<?php echo "B_ENTRY";' > "$SITE/b/entry.php"
printf '<?php echo "GONE_RAN";' > "$SITE/gone.php"
printf '<?php echo "UNREADABLE_RAN";' > "$SITE/unreadable.php"
cat > "$SITE/prepend.php" <<'PHP'
<?php
echo "PREPEND_RAN|";
if (isset($_GET['narrow'])) {
	ini_set('open_basedir', '/srv/site/a');
}
PHP
cat > "$SITE/a/driver.php" <<'PHP'
<?php
$m = $_GET['m'] ?? 'inc';
echo "[$m]";
switch ($m) {
	case 'inc':    $r = @include "/srv/site/b/entry.php"; break;
	case 'once':   $r = @include_once "/srv/site/a/../b/entry.php"; break;
	case 'onceln': $r = @include_once "/srv/site/a/lnk.php"; break;
	case 'req':    $r = @require_once "/srv/site/a/../b/entry.php"; break;
}
echo $r === false ? "REFUSED" : "|INCLUDED";
PHP
cat > "$SITE/fl.php" <<'PHP'
<?php
// The descriptors of this process that name the running script, with the
// status flags the kernel records for each (O_NONBLOCK is 04000).
$out = [];
foreach (glob('/proc/self/fd/*') as $f) {
	if (@readlink($f) === __FILE__) {
		$n = basename($f);
		preg_match('/flags:\s*(\d+)/', (string) @file_get_contents("/proc/self/fdinfo/$n"), $m);
		$out[] = "fd$n " . (octdec($m[1] ?? '0') & 04000 ? 'NONBLOCK' : 'blocking');
	}
}
echo $out ? implode(',', $out) : 'no-fd-for-the-script';
PHP
printf '<?php echo "OUTSIDE_EXEC_MARKER";' > "$OUT/secret.php"
printf '<?php echo "OUTSIDE_EXEC_MARKER";' > "$OUT/dir/c.php"
printf 'OUTSIDE_TEXT_MARKER\n' > "$OUT/plain.txt"
printf '<?php echo "ALLOWED_MARKER";' > "$OUT/allowed/ok.php"
# Old, distinct mtimes: OPcache will not cache a file younger than
# file_update_protection, and it recompiles only when the mtime changes.
touch -t 202001010000 "$SITE"/*.php "$SITE"/real/*.php "$OUT"/secret.php "$OUT"/dir/c.php "$OUT"/allowed/ok.php
touch -t 202002020000 "$SITE/inside2.php"
chmod -R a+rX "$TMP/fixture"

# Builds, inside a container, the three phar shapes phar_compile_file() treats
# differently for a script whose name contains ".phar": a tar-based and a
# zip-based one (the handle is replaced by the phar's stub stream) and a
# whole-file compressed one (the handle's stream is pointed at the phar).
cat > "$TMP/mkphars.php" <<'PHP'
<?php
$dir = '/srv/site';
foreach (['tar' => 'tar', 'zip' => 'zip'] as $kind => $ext) {
	$p = new Phar("$dir/$kind.phar.$ext", 0, "$kind.phar");
	$p['x.txt'] = 'x';
	$p->setStub('<?php echo "PHAR_' . strtoupper($kind) . '_STUB"; __HALT_COMPILER();');
	unset($p);
	rename("$dir/$kind.phar.$ext", "$dir/$kind.phar.php");
}
$g = new Phar("$dir/gz.phar");
$g['x.txt'] = 'x';
$g->setStub('<?php echo "PHAR_GZ_STUB"; __HALT_COMPILER();');
$c = $g->compress(Phar::GZ);
unset($g, $c);
rename("$dir/gz.phar.gz", "$dir/gz.phar.php");
unlink("$dir/gz.phar");
foreach (['tar', 'zip', 'gz'] as $kind) {
	chmod("$dir/$kind.phar.php", 0644);
}
PHP

NAME=""
PORT=""
STATUS=""
BODY=""

free_port() {
	python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()'
}

start_container() {
	# start_container <tag> <off|on> [extra docker-create args...]
	#
	# The document root and the "outside" tree live in anonymous volumes on the
	# daemon's own filesystem, not in bind mounts from the host: a link created
	# from the host would be resolved by the host's view of the path, and
	# Docker Desktop's file sharing adds its own caching to the picture. Links
	# are swapped with `docker exec -u root` (the image serves as www-data).
	local tag=$1 ini=$2
	shift 2
	NAME="symexec_${tag}_${SUFFIX}"
	PORT="$(free_port)"
	CONTAINERS+=("$NAME")
	docker create --name "$NAME" \
		-e DOCUMENT_ROOT=/srv/site \
		-e PHP_WORKERS=1 \
		-e LOG_LEVEL=info \
		"$@" \
		-p "${PORT}:80" \
		-v /srv/site -v /outside \
		"$IMAGE" >/dev/null || return 1
	docker cp "$TMP/${ini}.ini" "$NAME:/usr/local/etc/php/conf.d/zz-symlink.ini" >/dev/null || return 1
	docker cp "$TMP/fixture/site/." "$NAME:/srv/site" >/dev/null || return 1
	docker cp "$TMP/fixture/outside/." "$NAME:/outside" >/dev/null || return 1
	docker start "$NAME" >/dev/null || return 1
	for _ in $(seq 1 30); do
		get /ready.php 2
		[ "$STATUS" = 200 ] && return 0
		sleep 1
	done
	return 1
}

# get <path> [max-seconds] — sets STATUS and BODY. STATUS is 000 on timeout.
get() {
	STATUS="$(curl -s -o "$TMP/body" -w '%{http_code}' --max-time "${2:-10}" "http://localhost:${PORT}$1")"
	BODY="$(cat "$TMP/body" 2>/dev/null)"
}

# link <name> <target> — (re)points /srv/site/<name>, from inside the container.
link() {
	docker exec -u root "$NAME" ln -sfn "$2" "/srv/site/$1"
}

log_count() {
	docker logs "$NAME" 2>&1 | grep -c "$1"
}

# fd_count — open descriptors of the server process (workers are its threads).
fd_count() {
	docker exec -u root "$NAME" sh -c 'ls /proc/$(pidof oxphp | cut -d" " -f1)/fd | wc -l' | tr -d ' \r'
}

REFUSED='opened script escapes document root'

# serve_twice <label> <name> <first-target>
# Points the link inside the root and serves it twice: the second answer comes
# from the route cache.
serve_twice() {
	local label=$1 name=$2
	link "$name" "$3"
	get "/$name"
	if [ "$STATUS" = 200 ]; then ok "$label: the link serves before the swap"; else bad "$label: the link did not serve before the swap ($STATUS: $BODY)"; fi
	get "/$name"
}

# swap_and_check <label> <name> <marker> <outside-target>
# Repoints the link outside the root and serves it again.
swap_and_check() {
	local label=$1 name=$2 marker=$3 before after
	link "$name" "$4"
	before="$(log_count "$REFUSED")"
	get "/$name"
	if [[ "$BODY" == *"$marker"* ]]; then
		bad "$label: LEAK — the repointed link served a file outside the root ($STATUS: $BODY)"
	else
		ok "$label: nothing outside the root is served after the swap"
	fi
	if [ "$STATUS" = 404 ]; then ok "$label: the swapped link answers 404"; else bad "$label: expected 404, got $STATUS"; fi
	after="$(log_count "$REFUSED")"
	if [ "$after" -gt "$before" ]; then ok "$label: the refusal is logged by the opened-script check"; else bad "$label: no '$REFUSED' line in the log"; fi
}

# swapped_outside <label> <name> <marker> <first-target> <outside-target>
swapped_outside() {
	serve_twice "$1" "$2" "$4"
	swap_and_check "$1" "$2" "$3" "$5"
}

scenarios() {
	local label=$1 before after

	get /inside.php
	if [ "$STATUS" = 200 ] && [[ "$BODY" == *INSIDE_MARKER* ]]; then ok "$label: a plain script executes"; else bad "$label: a plain script did not execute ($STATUS: $BODY)"; fi

	# Control: a link that points outside from the start is refused at routing.
	link fromstart.php /outside/secret.php
	get /fromstart.php
	if [ "$STATUS" = 404 ] && [[ "$BODY" != *OUTSIDE_EXEC_MARKER* ]]; then ok "$label: a link outside the root from the start answers 404"; else bad "$label: from-start outside link answered $STATUS: $BODY"; fi

	swapped_outside "$label/leaf" leaf.php OUTSIDE_EXEC_MARKER /srv/site/inside.php /outside/secret.php
	swapped_outside "$label/text" text.php OUTSIDE_TEXT_MARKER /srv/site/inside.php /outside/plain.txt

	# A directory link in the middle of the path.
	link dir /srv/site/real
	get /dir/c.php
	if [ "$STATUS" = 200 ]; then ok "$label/dir: the path through the directory link serves"; else bad "$label/dir: did not serve before the swap ($STATUS)"; fi
	get /dir/c.php
	link dir /outside/dir
	before="$(log_count "$REFUSED")"
	get /dir/c.php
	if [[ "$BODY" == *OUTSIDE_EXEC_MARKER* ]]; then bad "$label/dir: LEAK — the repointed directory link executed a script outside the root"; else ok "$label/dir: nothing outside the root is served after the swap"; fi
	if [ "$STATUS" = 404 ]; then ok "$label/dir: the swapped path answers 404"; else bad "$label/dir: expected 404, got $STATUS"; fi
	after="$(log_count "$REFUSED")"
	if [ "$after" -gt "$before" ]; then ok "$label/dir: the refusal is logged by the opened-script check"; else bad "$label/dir: no '$REFUSED' line in the log"; fi

	# A swap that stays inside the root keeps working.
	link inside_swap.php inside.php
	get /inside_swap.php
	get /inside_swap.php
	link inside_swap.php inside2.php
	get /inside_swap.php
	if [ "$STATUS" = 200 ] && [[ "$BODY" == *INSIDE2_MARKER* ]]; then ok "$label/inside: a link repointed to another script inside the root executes it"; else bad "$label/inside: expected INSIDE2_MARKER, got $STATUS: $BODY"; fi

	# __FILE__ / __DIR__ / SCRIPT_FILENAME of a linked script.
	link ident_link.php /srv/site/real/ident.php
	get /ident_link.php
	if [ "$STATUS" = 200 ] && [ "$BODY" = "/srv/site/real/ident.php|/srv/site/ident_link.php|/srv/site/real" ]; then
		ok "$label/ident: __FILE__ is the resolved path, SCRIPT_FILENAME the requested one"
	else
		bad "$label/ident: got $STATUS: $BODY"
	fi

	# The engine changes into the directory of the file it was handed, which is
	# the checked, resolved one — not the directory the link lives in.
	link cwd_link.php /srv/site/real/cwd.php
	get /cwd_link.php
	if [ "$STATUS" = 200 ] && [ "$BODY" = "/srv/site/real|/srv/site/real" ]; then
		ok "$label/cwd: a linked script runs in the directory of the file that was checked"
	else
		bad "$label/cwd: got $STATUS: $BODY"
	fi
	# Where the link and its target share a directory, or the link is a
	# directory higher up the path, the working directory is what it always was.
	link cwd_same.php /srv/site/cwd_root.php
	get /cwd_same.php
	if [ "$STATUS" = 200 ] && [ "$BODY" = "/srv/site|/srv/site" ]; then ok "$label/cwd: a link to a file in its own directory keeps that directory"; else bad "$label/cwd: same-directory link got $STATUS: $BODY"; fi
	link cwd_dir /srv/site/real
	get /cwd_dir/cwd.php
	if [ "$STATUS" = 200 ] && [ "$BODY" = "/srv/site/real|/srv/site/real" ]; then ok "$label/cwd: a link in the middle of the path keeps the directory it leads to"; else bad "$label/cwd: directory link got $STATUS: $BODY"; fi
}

echo "== script symlink swap ($IMAGE) =="

# ── Default realpath cache: started first, finished last ─────
# The link is served now and repointed after the engine's 120 s realpath cache
# TTL has run out, while the other containers run.
echo "-- default realpath cache (swap after the TTL)"
TTL_READY=0
if start_container ttl default; then
	TTL_NAME="$NAME"
	TTL_PORT="$PORT"
	TTL_START="$(date +%s)"
	TTL_READY=1
	serve_twice "ttl" ttl.php /srv/site/inside.php

	# PHP's per-thread realpath cache remembers where a directory link led
	# while it pointed outside the root. After the link is repointed inside,
	# the engine must still change into the directory of the file the worker
	# checked, not into the one the cache remembers.
	link rpcdir /outside/dir
	get /rpc_driver.php
	link rpcdir /srv/site/real
	get /rpcdir/cwd.php
	if [ "$STATUS" = 200 ] && [ "$BODY" = "/srv/site/real|/srv/site/real" ]; then ok "ttl/cwd: a stale realpath-cache entry for a directory link does not decide the working directory"; else bad "ttl/cwd: got $STATUS: $BODY"; fi
else
	bad "ttl: container did not start"
fi

# ── OPcache off ──────────────────────────────────────────────
echo "-- opcache off"
if start_container off off; then
	scenarios "off"
else
	bad "off: container did not start"
	docker logs "$NAME" 2>&1 | tail -20
fi

# ── OPcache on, revalidating every request ───────────────────
echo "-- opcache on (validate_timestamps=1, revalidate_freq=0)"
if start_container on on; then
	scenarios "on"

	# A hot script keeps hitting the cache: the open the server now performs
	# must not make every request a compile.
	get /hits.php; get /hits.php; get /hits.php
	h3="$BODY"
	get /hits.php
	h4="$BODY"
	if [ "${h3%%|*}" -lt "${h4%%|*}" ] 2>/dev/null && [ "${h3##*|}" = "${h4##*|}" ]; then
		ok "on/hits: repeat requests hit OPcache without new misses ($h3 -> $h4)"
	else
		bad "on/hits: hits|misses went $h3 -> $h4"
	fi
else
	bad "on: container did not start"
	docker logs "$NAME" 2>&1 | tail -20
fi

# ── OPcache on, never revalidating ───────────────────────────
# A script that is already compiled is answered from memory without the engine
# opening the file, so the engine alone would not notice a swap here. The
# refusal must not depend on that: the worker opens the file regardless.
echo "-- opcache on (validate_timestamps=0, script already cached)"
if start_container cached cached; then
	swapped_outside "cached/leaf" leaf.php OUTSIDE_EXEC_MARKER /srv/site/inside.php /outside/secret.php

	# A link repointed to another script inside the root, with the first one
	# already compiled and never revalidated: the engine must run the file the
	# worker checked, and list that one file — not the cached one as well.
	link incl.php /srv/site/incl_a.php
	get /incl.php
	get /incl.php
	if [ "$STATUS" = 200 ] && [ "$BODY" = "INCL_A|1" ]; then ok "cached/inside: the first target runs and is the only included file"; else bad "cached/inside: before the swap got $STATUS: $BODY"; fi
	link incl.php /srv/site/incl_b.php
	get /incl.php
	if [ "$STATUS" = 200 ] && [ "$BODY" = "INCL_B|1" ]; then ok "cached/inside: a link repointed inside the root runs the new target, one included file"; else bad "cached/inside: after the swap got $STATUS: $BODY (expected INCL_B|1)"; fi

	# A script that `include`s a link while it points outside the root caches
	# the outside file under the link's own path as an alias. Once the link
	# points inside again, a request for it must run what the worker checked,
	# not what the alias leads to. (The include itself is not covered; this
	# only pins what the primary script does afterwards.)
	link poison.php /outside/secret.php
	get /include_driver.php
	link poison.php /srv/site/inside.php
	get /poison.php
	if [ "$STATUS" = 200 ] && [ "$BODY" = "INSIDE_MARKER" ]; then ok "cached/alias: a link that was included while pointing outside runs the checked file once it points inside"; else bad "cached/alias: expected INSIDE_MARKER, got $STATUS: $BODY"; fi

	# Scripts reached through links keep hitting the cache, whichever link they
	# are reached by.
	link hits_a.php /srv/site/hits.php
	link hits_b.php /srv/site/hits.php
	get /hits_a.php; get /hits_a.php; get /hits_b.php
	c3="$BODY"
	get /hits_a.php; get /hits_b.php; get /hits_a.php
	c4="$BODY"
	# The three requests between the two readings are three hits and no miss.
	if [ "${c4%%|*}" -ge $((${c3%%|*} + 3)) ] 2>/dev/null && [ "${c3##*|}" = "${c4##*|}" ]; then
		ok "cached/hits: scripts reached through two links keep hitting OPcache without new misses ($c3 -> $c4)"
	else
		bad "cached/hits: hits|misses went $c3 -> $c4"
	fi
else
	bad "cached: container did not start"
	docker logs "$NAME" 2>&1 | tail -20
fi

# ── FIFO swapped in ──────────────────────────────────────────
echo "-- fifo"
if start_container fifo off; then
	link fifo.php /srv/site/inside.php
	get /fifo.php
	get /fifo.php
	docker exec -u root "$NAME" mkfifo /outside/fifo
	link fifo.php /outside/fifo
	get /fifo.php 8
	if [ "$STATUS" = 404 ]; then ok "fifo: a link repointed at a FIFO outside the root answers 404 without blocking"; else bad "fifo: expected 404, got $STATUS (000 = timed out, the worker is blocked in open)"; fi
	get /inside.php 8
	if [ "$STATUS" = 200 ]; then ok "fifo: the single worker still serves afterwards"; else bad "fifo: the worker is wedged ($STATUS)"; fi

	# Inside the root the location check passes, so the refusal has to come
	# from the file type: a FIFO would read as an empty script and a directory
	# as another.
	docker exec -u root "$NAME" mkfifo /srv/site/pipe
	docker exec -u root "$NAME" mkdir /srv/site/adir
	for kind in "pipe:a FIFO" "adir:a directory"; do
		link fifo.php "/srv/site/${kind%%:*}"
		before="$(docker logs "$NAME" 2>&1 | grep -c 'Cannot open script')"
		get /fifo.php 8
		after="$(docker logs "$NAME" 2>&1 | grep -c 'Cannot open script')"
		if [ "$STATUS" = 500 ] && [ "$after" -gt "$before" ]; then ok "fifo: a link repointed at ${kind#*:} inside the root answers 500 and is logged"; else bad "fifo: ${kind#*:} inside the root answered $STATUS (log lines $before -> $after)"; fi
	done
	get /inside.php 8
	if [ "$STATUS" = 200 ]; then ok "fifo: the single worker still serves after both"; else bad "fifo: the worker is wedged after the inside-root cases ($STATUS)"; fi
else
	bad "fifo: container did not start"
fi

# ── SYMLINK_ALLOW_PATHS ──────────────────────────────────────
echo "-- SYMLINK_ALLOW_PATHS=/outside/allowed"
if start_container allow off -e SYMLINK_ALLOW_PATHS=/outside/allowed; then
	link allowed.php /srv/site/inside.php
	get /allowed.php
	get /allowed.php
	link allowed.php /outside/allowed/ok.php
	get /allowed.php
	if [ "$STATUS" = 200 ] && [[ "$BODY" == *ALLOWED_MARKER* ]]; then ok "allow: a link repointed at an allow-listed target executes it"; else bad "allow: expected ALLOWED_MARKER, got $STATUS: $BODY"; fi
	link allowed.php /outside/secret.php
	get /allowed.php
	if [ "$STATUS" = 404 ] && [[ "$BODY" != *OUTSIDE_EXEC_MARKER* ]]; then ok "allow: a sibling outside the allow-list is still refused"; else bad "allow: sibling answered $STATUS: $BODY"; fi
else
	bad "allow: container did not start"
fi

# ── The open file as the engine sees it ──────────────────────
# Opcache is off so that every request compiles the script, which is where the
# phar extension looks at the handle it is given.
echo "-- script handle (phar, read errors)"
if start_container stream off --cap-add SYS_ADMIN; then
	docker cp "$TMP/mkphars.php" "$NAME:/tmp/mkphars.php" >/dev/null
	if docker exec -u root "$NAME" php -d phar.readonly=0 /tmp/mkphars.php; then
		for kind in tar zip gz; do
			get "/${kind}.phar.php"
			first="$STATUS"
			# A compressed phar runs the default stub, which has nothing to
			# print; the other two print the stub the fixture set.
			marker="PHAR_$(echo "$kind" | tr a-z A-Z)_STUB"
			[ "$kind" = gz ] && marker=""
			before="$(fd_count)"
			for _ in $(seq 1 40); do get "/${kind}.phar.php"; done
			after="$(fd_count)"
			if [ "$first" = 200 ] && [ "$STATUS" = 200 ] && [[ "$BODY" == *"$marker"* ]]; then ok "stream/$kind: a $kind phar named *.phar.php runs"; else bad "stream/$kind: got $first / $STATUS: $BODY (expected 200 and '$marker')"; fi
			if [ -n "$before" ] && [ -n "$after" ] && [ "$after" -le $((before + 3)) ]; then
				ok "stream/$kind: 40 requests leave the descriptor count where it was ($before -> $after)"
			else
				bad "stream/$kind: descriptors went $before -> $after over 40 requests — the script's file is not released"
			fi
		done
	else
		bad "stream: could not build the phar fixtures"
	fi

	# The descriptor PHP reads the script from is in blocking mode. The worker
	# opens it with O_NONBLOCK to keep a FIFO from blocking the open; PHP's
	# stream layer takes EAGAIN for "no data yet", which the engine reads as
	# the end of the file, so a non-blocking descriptor could run a truncated
	# script as a success on a filesystem that answers that way.
	get /fl.php
	if [ "$STATUS" = 200 ] && [[ "$BODY" == fd* ]] && [[ "$BODY" != *NONBLOCK* ]]; then ok "stream/blocking: the descriptor the script is read from is in blocking mode ($BODY)"; else bad "stream/blocking: expected a blocking descriptor, got $STATUS: $BODY"; fi

	# The phar extension takes over a script by the ".phar" in the name it is
	# given, and that name is the resolved one. A link whose own name has no
	# ".phar" to a phar that does runs the phar (it used to print the archive's
	# bytes); a ".phar" link to a file whose name has none is no longer taken
	# for one. Both are documented behavior changes.
	for kind in tar zip; do
		link "viaphar_${kind}.php" "/srv/site/${kind}.phar.php"
		get "/viaphar_${kind}.php"
		marker="PHAR_$(echo "$kind" | tr a-z A-Z)_STUB"
		# Run as a phar the script prints its stub's output and nothing else; compiled
		# as a plain file the archive's own bytes come first (the stub sits inside it).
		if [ "$STATUS" = 200 ] && [ "$BODY" = "$marker" ]; then ok "stream/phar-name: a link named *.php to a $kind phar runs the phar"; else bad "stream/phar-name: link to a $kind phar answered $STATUS: ${BODY:0:80} (expected exactly '$marker')"; fi
	done
	# A compressed phar run as a phar answers with whatever its default stub
	# makes of it; compiled as a plain file it answers with the compressed
	# bytes, which start with the gzip magic number 1f 8b.
	link viaphar_gz.php /srv/site/gz.phar.php
	get /viaphar_gz.php
	if [ "$STATUS" = 200 ] && [ "$(head -c 2 "$TMP/body" | od -An -tx1 | tr -d ' \n')" != 1f8b ]; then ok "stream/phar-name: a link named *.php to a compressed phar runs the phar"; else bad "stream/phar-name: link to a compressed phar answered $STATUS with the archive's bytes"; fi
	docker exec -u root "$NAME" sh -c 'cp /srv/site/zip.phar.php /srv/site/zip.bin && chmod a+r /srv/site/zip.bin'
	link zipbin.phar.php /srv/site/zip.bin
	get /zipbin.phar.php
	if [ "$STATUS" = 200 ] && [ "$(head -c 2 "$TMP/body")" = PK ]; then ok "stream/phar-name: a *.phar.php link to a file whose own name has no .phar is not taken for a phar"; else bad "stream/phar-name: expected the archive's bytes (PK...), got $STATUS: ${BODY:0:80}"; fi
	docker exec -u root "$NAME" sh -c 'cp /srv/site/gz.phar.php /srv/site/gz.bin && chmod a+r /srv/site/gz.bin'
	link gzbin.phar.php /srv/site/gz.bin
	get /gzbin.phar.php
	if [ "$STATUS" = 200 ] && [ "$(head -c 2 "$TMP/body" | od -An -tx1 | tr -d ' \n')" = 1f8b ]; then ok "stream/phar-name: the same for a compressed phar"; else bad "stream/phar-name: expected the compressed bytes (1f8b...), got $STATUS: ${BODY:0:80}"; fi

	# /proc/<pid>/mem is a regular file whose read at offset 0 fails with EIO.
	# stdio's fread reports that as end of file, so the engine would compile
	# what it had read — nothing — and answer 200 with an empty body; a read
	# error has to fail the request. SYMLINK_ALLOW_PATHS refuses /proc, so the
	# file is bind-mounted over a path inside the root.
	if docker exec -u root "$NAME" sh -c ': > /srv/site/mem.php && chmod a+r /srv/site/mem.php && mount --bind "/proc/$(pidof oxphp | cut -d" " -f1)/mem" /srv/site/mem.php'; then
		get /mem.php
		if [ "$STATUS" = 500 ]; then ok "stream/eio: a script whose read fails answers 500"; else bad "stream/eio: expected 500, got $STATUS (200 with an empty body = the read error was taken for end of file): $BODY"; fi
		get /inside.php
		if [ "$STATUS" = 200 ]; then ok "stream/eio: the worker still serves afterwards"; else bad "stream/eio: the worker does not serve afterwards ($STATUS)"; fi
	else
		bad "stream/eio: could not bind-mount /proc/<pid>/mem (needs CAP_SYS_ADMIN)"
	fi
else
	bad "stream: container did not start"
	docker logs "$NAME" 2>&1 | tail -20
fi

# ── open_basedir ─────────────────────────────────────────────
# The engine applies open_basedir where it opens a script by name, and a script
# handed over as an open file never gets there, so the worker applies it itself
# to the path it checked. A refusal reads as the engine's own for a script it
# cannot open: two warnings, then a fatal error, status 500.
for rp in 0 1; do
echo "-- open_basedir (opcache.revalidate_path=$rp)"
if start_container "obd$rp" "obd$rp"; then
	get /b/entry.php
	if [ "$STATUS" = 500 ] && [[ "$BODY" == *"Unknown: open_basedir restriction in effect. File(/srv/site/b/entry.php) is not within the allowed path(s): (/srv/site/a:/srv/site/ready.php)"* ]] \
		&& [[ "$BODY" == *"Unknown: Failed to open stream: Operation not permitted"* ]] \
		&& [[ "$BODY" == *"Failed opening required '/srv/site/b/entry.php' (include_path="* ]] && [[ "$BODY" != *B_ENTRY* ]]; then
		ok "obd$rp/entry: an entry script outside open_basedir answers 500 with the engine's refusal"
	else
		bad "obd$rp/entry: expected 500 and the engine's three messages, got $STATUS: ${BODY:0:400}"
	fi
	if [ "$(log_count 'PHP: Unknown: open_basedir restriction in effect')" -ge 1 ] && [ "$(log_count "PHP: Failed opening required '/srv/site/b/entry.php'")" -ge 1 ]; then ok "obd$rp/entry: the refusal is logged"; else bad "obd$rp/entry: the refusal is not in the log"; fi
	get /b/entry.php
	if [ "$STATUS" = 500 ]; then ok "obd$rp/entry: refused again on the next request"; else bad "obd$rp/entry: the second request answered $STATUS: ${BODY:0:200}"; fi

	link a/lnk.php /srv/site/b/entry.php
	get /a/lnk.php
	if [ "$STATUS" = 500 ] && [[ "$BODY" == *"open_basedir restriction in effect. File(/srv/site/b/entry.php)"* ]] && [[ "$BODY" != *B_ENTRY* ]]; then ok "obd$rp/link: a link inside the root that resolves outside open_basedir is refused"; else bad "obd$rp/link: expected the refusal, got $STATUS: ${BODY:0:300}"; fi

	get /a/inside.php
	if [ "$STATUS" = 200 ] && [ "$BODY" = OBD_INSIDE ]; then ok "obd$rp/inside: a script inside open_basedir runs"; else bad "obd$rp/inside: got $STATUS: ${BODY:0:200}"; fi

	# Nothing outside was cached by the refused entry runs, so no include form
	# finds it: each refuses, as it does before any entry run.
	for m in inc once onceln req; do
		get "/a/driver.php?m=$m"
		# A refused include answers false; a refused require is a fatal error.
		refused=0
		if [ "$m" = req ]; then [ "$STATUS" = 500 ] && [[ "$BODY" == *"Failed opening required"* ]] && refused=1; else [[ "$BODY" == *REFUSED* ]] && refused=1; fi
		if [ "$refused" = 1 ] && [[ "$BODY" != *B_ENTRY* ]]; then ok "obd$rp/include-$m: an outside file is still refused after the entry attempts"; else bad "obd$rp/include-$m: the outside file was included ($STATUS: ${BODY:0:200})"; fi
	done
	get /a/inside.php
	if [ "$STATUS" = 200 ]; then ok "obd$rp: the worker serves afterwards"; else bad "obd$rp: the worker does not serve afterwards ($STATUS)"; fi
else
	bad "obd$rp: container did not start"
	docker logs "$NAME" 2>&1 | tail -20
fi
done

# ── open_basedir where the engine applies it ─────────────────
# The engine checks open_basedir when it compiles the script: after
# php_execute_script() has changed into the script's directory and run
# auto_prepend_file. The worker's own check is made at that moment too, so a
# relative entry resolves against the same directory and a narrowed open_basedir
# is the one that counts.
for wd in default /tmp; do
	extra=()
	[ "$wd" != default ] && extra=(-w "$wd")
	echo "-- open_basedir=. (process working directory: $wd)"
	if start_container "obdot$(basename "$wd")" obdot ${extra[@]+"${extra[@]}"}; then
		get /inside.php
		if [ "$STATUS" = 200 ] && [ "$BODY" = INSIDE_MARKER ]; then ok "obdot/$wd: '.' is the directory of the script, not the process's"; else bad "obdot/$wd: a script in the directory '.' names answered $STATUS: ${BODY:0:300}"; fi
		get /a/inside.php
		if [ "$STATUS" = 200 ] && [ "$BODY" = OBD_INSIDE ]; then ok "obdot/$wd: '.' follows the script into a subdirectory"; else bad "obdot/$wd: a script in a subdirectory answered $STATUS: ${BODY:0:300}"; fi
	else
		bad "obdot/$wd: container did not start (its readiness script is refused?)"
		docker logs "$NAME" 2>&1 | tail -8 | cut -c1-300
	fi
done

echo "-- open_basedir narrowed by auto_prepend_file"
if start_container obdpre obdpre; then
	get "/inside.php?narrow=1"
	if [ "$STATUS" = 500 ] && [[ "$BODY" == "PREPEND_RAN|"* ]] \
		&& [[ "$BODY" == *"open_basedir restriction in effect. File(/srv/site/inside.php) is not within the allowed path(s): (/srv/site/a)"* ]] \
		&& [[ "$BODY" == *"Failed opening required '/srv/site/inside.php'"* ]] && [[ "$BODY" != *INSIDE_MARKER* ]]; then
		ok "obdpre: the prepended file runs first, then the narrowed open_basedir refuses the script"
	else
		bad "obdpre: expected PREPEND_RAN| and the refusal, got $STATUS: ${BODY:0:400}"
	fi
	get "/a/inside.php?narrow=1"
	if [ "$STATUS" = 200 ] && [ "$BODY" = "PREPEND_RAN|OBD_INSIDE" ]; then ok "obdpre: a script inside the narrowed open_basedir runs"; else bad "obdpre: got $STATUS: ${BODY:0:300}"; fi
	get /inside.php
	if [ "$STATUS" = 200 ] && [ "$BODY" = "PREPEND_RAN|INSIDE_MARKER" ]; then ok "obdpre: without the narrowing the script runs, after the prepended file"; else bad "obdpre: got $STATUS: ${BODY:0:300}"; fi
	# inside.php is compiled and held by OPcache now, which would answer it
	# without opening it.
	get "/inside.php?narrow=1"
	if [ "$STATUS" = 500 ] && [[ "$BODY" == *"open_basedir restriction in effect. File(/srv/site/inside.php)"* ]] && [[ "$BODY" != *INSIDE_MARKER* ]]; then ok "obdpre: a script OPcache holds is refused too"; else bad "obdpre: the narrowed open_basedir did not refuse a cached script ($STATUS: ${BODY:0:300})"; fi
	get /inside.php
	if [ "$STATUS" = 200 ]; then ok "obdpre: the worker serves afterwards"; else bad "obdpre: the worker does not serve afterwards ($STATUS)"; fi
else
	bad "obdpre: container did not start"
	docker logs "$NAME" 2>&1 | tail -20
fi

# The same open_basedir that refuses the entry script outright, with a prepended
# file in effect: the prepended file has run by the time the refusal comes.
echo "-- the prepended file runs before a refused script"
if start_container obdpre2 obdpre2; then
	get /b/entry.php
	if [ "$STATUS" = 500 ] && [[ "$BODY" == "PREPEND_RAN|"* ]] \
		&& [[ "$BODY" == *"open_basedir restriction in effect. File(/srv/site/b/entry.php)"* ]] && [[ "$BODY" != *B_ENTRY* ]]; then
		ok "obdpre2: an entry script outside open_basedir is refused after the prepended file ran"
	else
		bad "obdpre2: expected PREPEND_RAN| before the refusal, got $STATUS: ${BODY:0:300}"
	fi
	get /a/inside.php
	if [ "$STATUS" = 200 ] && [ "$BODY" = "PREPEND_RAN|OBD_INSIDE" ]; then ok "obdpre2: a script inside open_basedir runs after the prepended file"; else bad "obdpre2: got $STATUS: ${BODY:0:300}"; fi
else
	bad "obdpre2: container did not start"
	docker logs "$NAME" 2>&1 | tail -20
fi

# ── Scripts that cannot be opened ────────────────────────────
# The worker opens the script itself, so one that cannot be opened is answered
# before PHP starts. PHP would have reported its own failed open, but with
# opcache.validate_timestamps=0 it does not open a script it already holds, and
# runs the cached copy of a script deleted since.
echo "-- scripts that cannot be opened"
if start_container deploy cached; then
	get /gone.php
	if [ "$STATUS" = 200 ] && [ "$BODY" = GONE_RAN ]; then ok "deploy/gone: the script runs and OPcache compiles it"; else bad "deploy/gone: got $STATUS: ${BODY:0:200}"; fi
	docker exec -u root "$NAME" rm /srv/site/gone.php
	get /gone.php
	if [ "$STATUS" = 404 ] && [[ "$BODY" != *GONE_RAN* ]]; then ok "deploy/gone: a script deleted after it was compiled answers 404"; else bad "deploy/gone: expected 404, got $STATUS: ${BODY:0:200}"; fi
	docker exec -u root "$NAME" chmod 000 /srv/site/unreadable.php
	get /unreadable.php
	if [ "$STATUS" = 500 ] && [ "$BODY" = "Internal Server Error" ] && [ "$(log_count 'Cannot open script')" -ge 1 ]; then ok "deploy/unreadable: an unreadable script answers a plain 500 and is logged"; else bad "deploy/unreadable: expected a plain 500 and the log line, got $STATUS: ${BODY:0:200}"; fi
else
	bad "deploy: container did not start"
	docker logs "$NAME" 2>&1 | tail -20
fi

# ── Finish the default-TTL scenario ──────────────────────────
if [ "$TTL_READY" = 1 ]; then
	echo "-- default realpath cache: swapping after the TTL"
	NAME="$TTL_NAME"
	PORT="$TTL_PORT"
	wait_for=$((TTL_START + 125 - $(date +%s)))
	[ "$wait_for" -gt 0 ] && sleep "$wait_for"
	swap_and_check "ttl" ttl.php OUTSIDE_EXEC_MARKER /outside/secret.php
fi

echo
echo "passed: $PASS  failed: $FAIL"
[ "$FAIL" -eq 0 ]
