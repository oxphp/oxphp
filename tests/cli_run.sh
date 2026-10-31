#!/usr/bin/env bash
#
# Integration smoke test for the one-shot CLI role: `oxphp run <script.php>`.
#
# Runs the fixtures in tests/fixtures/cli against a built OxPHP image and
# asserts the acceptance criteria (stdout, exit code, PHP_SAPI, $argv/$argc,
# -d ini overrides, async/shared availability, stderr routing) plus a light
# regression check that the HTTP `serve` role still boots.
#
# Usage: tests/cli_run.sh [IMAGE_REF]   (default: oxphp-oxphp:latest)
set -u

IMAGE="${1:-oxphp-oxphp:latest}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
FIX="$ROOT/tests/fixtures/cli"
PASS=0
FAIL=0

run() {
	# run <script.php> [args...] — prints stdout, returns container exit code.
	local script="$1"
	shift
	docker run --rm -v "$FIX:/cli:ro" "$IMAGE" oxphp run "/cli/$script" "$@"
}

ok()   { printf '  \033[32mPASS\033[0m %s\n' "$1"; PASS=$((PASS + 1)); }
bad()  { printf '  \033[31mFAIL\033[0m %s\n' "$1"; FAIL=$((FAIL + 1)); }

echo "== oxphp run smoke ($IMAGE) =="

# 1. stdout + exit 0
out="$(run hello.php)"; code=$?
[ "$out" = "hello from oxphp run" ] && [ $code -eq 0 ] \
	&& ok "hello: stdout + exit 0" || bad "hello (out='$out' code=$code)"

# 2. PHP_SAPI === 'cli'
out="$(run sapi.php)"
[ "$out" = "cli" ] && ok "PHP_SAPI == cli" || bad "PHP_SAPI (got '$out')"

# 3. \$argv / \$argc passthrough (incl. script flags reaching PHP)
out="$(run argv.php a b --verbose)"
argc="$(printf '%s\n' "$out" | sed -n 1p)"
argv="$(printf '%s\n' "$out" | sed -n 2p)"
{ [ "$argc" = "4" ] && [ "$argv" = "/cli/argv.php|a|b|--verbose" ]; } \
	&& ok "\$argv/\$argc + flag passthrough" || bad "argv (argc='$argc' argv='$argv')"

# 4. exit code propagation
run exitcode.php >/dev/null; code=$?
[ $code -eq 7 ] && ok "exit code propagation (7)" || bad "exit code (got $code)"

# 5. -d ini override
out="$(docker run --rm -v "$FIX:/cli:ro" "$IMAGE" oxphp run -d memory_limit=256M /cli/ini.php)"
[ "$out" = "256M" ] && ok "-d memory_limit override" || bad "-d override (got '$out')"

# 5b. CLI ini defaults from the ini_entries blob apply (max_execution_time=0)
#     — guards against the sapi_startup ini_entries drop. argc needs the blob
#     only on PHP 8.4 with a php.ini that turns register_argc_argv off; 5d
#     covers that.
out="$(run cliini.php)"
met="$(printf '%s' "$out" | cut -d'|' -f1)"
argc="$(printf '%s' "$out" | cut -d'|' -f2)"
{ [ "$met" = "0" ] && [ "$argc" != "NO-ARGC" ]; } \
	&& ok "CLI ini defaults (max_execution_time=0, argc)" || bad "cli ini defaults (got '$out')"

# 5c. -d works for a PHP_INI_PERDIR directive (config stage, not runtime alter)
out="$(docker run --rm -v "$FIX:/cli:ro" "$IMAGE" oxphp run -d max_input_vars=1234 /cli/inivar.php)"
[ "$out" = "1234" ] && ok "-d for PHP_INI_PERDIR (max_input_vars)" || bad "-d PERDIR (got '$out')"

# 5d. A php.ini that turns register_argc_argv off does not take $argv away: on
#     PHP 8.4 the run defaults turn it back on, and 8.5 builds $argv without it.
out="$(docker run --rm -v "$FIX:/cli:ro" -v "$FIX/argc_off.ini:/usr/local/etc/php/conf.d/zz-argc-off.ini:ro" "$IMAGE" oxphp run /cli/argv.php a)"
[ "$out" = "$(printf '2\n/cli/argv.php|a')" ] \
	&& ok "\$argv with php.ini register_argc_argv=Off" || bad "argv under ini Off (got '$out')"

# 6. async engine available under one-shot
out="$(run async.php)"
printf '%s' "$out" | grep -q "async-ok" && ok "async (oxphp_sleep)" || bad "async (got '$out')"

# 6b. With an async pool every pool thread opens a PHP request under the CLI
#     SAPI as it starts, whether or not the script dispatches anything.
out="$(docker run --rm -e ASYNC_WORKERS=1 -v "$FIX:/cli:ro" "$IMAGE" oxphp run /cli/hello.php)"; code=$?
{ [ "$out" = "hello from oxphp run" ] && [ $code -eq 0 ]; } \
	&& ok "ASYNC_WORKERS=1: script without tasks exits 0" || bad "pool idle (out='$out' code=$code)"

# 6c. A task round-trips through a pool thread, the idle second thread stops
#     cleanly, and the thread that runs the task writes nothing to stderr as its
#     request starts: it starts before the await returns, while stderr is open.
#     OPcache is off, as `run` has it without opcache.enable_cli=1 (the release
#     image ships none), so the task's code runs from the script's own memory.
aerr_file="$(mktemp)"
aout="$(docker run --rm -e ASYNC_WORKERS=2 -v "$FIX:/cli:ro" "$IMAGE" oxphp run -d opcache.enable_cli=0 /cli/async_dispatch.php 2>"$aerr_file")"; acode=$?
aerr="$(cat "$aerr_file")"; rm -f "$aerr_file"
{ [ "$aout" = "42" ] && [ $acode -eq 0 ] && [ -z "$aerr" ]; } \
	&& ok "ASYNC_WORKERS=2: oxphp_async() result + exit 0 + clean stderr" \
	|| bad "async dispatch (out='$aout' code=$acode stderr='$aerr')"

# 7. ox_shared classes registered
out="$(run shared.php)"
printf '%s' "$out" | grep -q "shared-ok" && ok "ox_shared classes" || bad "shared (got '$out')"

# 7b. Registry::pool() works for the whole script, and the idle slot it leaves
#     does not outlive the request: the process exits 0 with a clean heap.
perr_file="$(mktemp)"
pout="$(docker run --rm -v "$FIX:/cli:ro" "$IMAGE" oxphp run /cli/pool.php 2>"$perr_file")"; pcode=$?
perr="$(cat "$perr_file")"; rm -f "$perr_file"
{ [ "$pout" = "pool-ok idle=1" ] && [ $pcode -eq 0 ] && ! printf '%s' "$perr" | grep -q "zend_mm_heap corrupted"; } \
	&& ok "Registry::pool in one-shot + clean exit" || bad "pool one-shot (out='$pout' code=$pcode stderr='$perr')"

# 8. errors → stderr, not stdout
sout="$(docker run --rm -v "$FIX:/cli:ro" "$IMAGE" oxphp run /cli/stderr.php 2>/dev/null)"
serr="$(docker run --rm -v "$FIX:/cli:ro" "$IMAGE" oxphp run /cli/stderr.php 2>&1 1>/dev/null)"
{ [ "$sout" = "on-stdout" ] && printf '%s' "$serr" | grep -q "on-stderr"; } \
	&& ok "errors routed to stderr" || bad "stderr routing (stdout='$sout' stderr='$serr')"

# 8b. fatal error (uncaught Error) -> exit 255 (php-cli parity), error on stderr
fout="$(run fatal.php 2>/dev/null)"; fcode=$?
ferr="$(run fatal.php 2>&1 1>/dev/null)"
{ [ $fcode -eq 255 ] && printf '%s' "$fout" | grep -q "before-fatal" \
	&& printf '%s' "$ferr" | grep -qi "this_function_does_not_exist"; } \
	&& ok "fatal -> exit 255 + stderr" || bad "fatal (code=$fcode stdout='$fout' stderr='$ferr')"

# 8c. SUPERGLOBALS_ENABLED honored: default (true) folds process env into
#     $_SERVER; =false skips the fold but keeps the script skeleton and $argv.
sg_default="$(docker run --rm -e OXPHP_SG_PROBE=present -v "$FIX:/cli:ro" "$IMAGE" oxphp run /cli/superglobals.php)"
sg_argc="$(printf '%s\n' "$sg_default" | sed -n 1p)"
sg_skel="$(printf '%s\n' "$sg_default" | sed -n 2p)"
sg_env="$(printf '%s\n' "$sg_default" | sed -n 3p)"
{ [ "$sg_argc" = "1|/cli/superglobals.php" ] && [ "$sg_skel" = "skel-yes" ] && [ "$sg_env" = "env-yes" ]; } \
	&& ok "SUPERGLOBALS_ENABLED default: env folded + skeleton + argv" \
	|| bad "sg default (argc='$sg_argc' skel='$sg_skel' env='$sg_env')"

sg_off="$(docker run --rm -e OXPHP_SG_PROBE=present -e SUPERGLOBALS_ENABLED=false -v "$FIX:/cli:ro" "$IMAGE" oxphp run /cli/superglobals.php)"
off_argc="$(printf '%s\n' "$sg_off" | sed -n 1p)"
off_skel="$(printf '%s\n' "$sg_off" | sed -n 2p)"
off_env="$(printf '%s\n' "$sg_off" | sed -n 3p)"
{ [ "$off_argc" = "1|/cli/superglobals.php" ] && [ "$off_skel" = "skel-yes" ] && [ "$off_env" = "env-no" ]; } \
	&& ok "SUPERGLOBALS_ENABLED=false: env fold skipped, skeleton + argv kept" \
	|| bad "sg off (argc='$off_argc' skel='$off_skel' env='$off_env')"

# 8d. implicit run: `oxphp <script>` with no `run` keyword.
out="$(docker run --rm -v "$FIX:/cli:ro" "$IMAGE" oxphp /cli/hello.php)"; code=$?
{ [ "$out" = "hello from oxphp run" ] && [ $code -eq 0 ]; } \
	&& ok "implicit run (no 'run' keyword)" || bad "implicit run (out='$out' code=$code)"

# 8e. extensionless shebang script: implicit run + CG(skip_shebang). The `#!`
#     line must NOT leak into output.
out="$(docker run --rm -v "$FIX:/cli:ro" "$IMAGE" oxphp /cli/shebang)"
[ "$out" = "shebang-ok" ] && ok "shebang skipped (extensionless)" \
	|| bad "shebang (got '$out' — leaked shebang means CG(skip_shebang) not set)"

# 8f. --user on the run path: force the container to start as root (the image's
#     default user is www-data), oxphp drops to uid 82, the script sees the
#     dropped uid.
out="$(docker run --rm --user 0:0 -v "$FIX:/cli:ro" "$IMAGE" oxphp --user=82 /cli/whoami.php)"
[ "$out" = "82" ] && ok "--user drop on run (uid 82)" || bad "--user run drop (got '$out')"

# 8g. --user while NOT root is a hard error (exit 1), never a silent no-op — the
#     image's default user is www-data (non-root), so no --user override here.
gerr="$(docker run --rm -v "$FIX:/cli:ro" "$IMAGE" oxphp --user=82 /cli/whoami.php 2>&1 1>/dev/null)"; gcode=$?
{ [ $gcode -ne 0 ] && printf '%s' "$gerr" | grep -qi "root"; } \
	&& ok "--user as non-root is a hard error" || bad "--user non-root guard (code=$gcode err='$gerr')"

# 8h. PHP_BINARY is this oxphp binary, and a child started through it with the
#     command line amphp/parallel builds (-d flags, script, args) runs as a CLI
#     script with the same PHP_BINARY and the child's own -d applied.
exe="$(docker run --rm "$IMAGE" sh -c 'readlink -f "$(command -v oxphp)"')"
out="$(run php_binary.php 2>&1)"
parent="$(printf '%s\n' "$out" | sed -n 1p)"
child="$(printf '%s\n' "$out" | sed -n 2p)"
{ [ -n "$exe" ] && [ "${parent%|*}" = "parent|$exe" ] && [ "${child%|*}" = "child|cli|$exe|77M" ] \
	&& [ "${child##*|}" != "${parent##*|}" ]; } \
	&& ok "PHP_BINARY = oxphp, child started through it" || bad "PHP_BINARY (exe='$exe' out='$out')"

# 8i. The working directory stays the one oxphp was started in, as under php:
#     a script path with a directory in it opens, and getcwd() is the launch
#     directory, not the script's. Once more with OPcache off, as the release
#     image has it under run.
for opc in "" "-d opcache.enable_cli=0"; do
	# shellcheck disable=SC2086 # $opc is empty or two words on purpose
	out="$(docker run --rm -w /cli -v "$FIX:/cli:ro" "$IMAGE" oxphp run $opc sub/cwd.php 2>&1)"; code=$?
	{ [ "$out" = "/cli/sub/cwd.php|/cli" ] && [ $code -eq 0 ]; } \
		&& ok "relative path with a directory${opc:+ ($opc)}" || bad "relative path${opc:+ ($opc)} (out='$out' code=$code)"
done
out="$(docker run --rm -w /cli -v "$FIX:/cli:ro" "$IMAGE" oxphp ./sub/cwd.php 2>&1)"; code=$?
{ [ "$out" = "/cli/sub/cwd.php|/cli" ] && [ $code -eq 0 ]; } \
	&& ok "implicit run of ./sub/cwd.php" || bad "implicit ./ path (out='$out' code=$code)"
out="$(docker run --rm -w /tmp -v "$FIX:/cli:ro" "$IMAGE" oxphp run /cli/sub/cwd.php 2>&1)"; code=$?
{ [ "$out" = "/cli/sub/cwd.php|/tmp" ] && [ $code -eq 0 ]; } \
	&& ok "absolute path: getcwd() is the launch directory" || bad "absolute path cwd (out='$out' code=$code)"

# 8j. The script is opened by its path, as php opens it: not looked up in
#     include_path (incpath/ holds a decoy sub/cwd.php) and not checked against
#     open_basedir. `__FILE__` of a symlink names its target, as under php. As
#     under the image's php, which is ZTS (an NTS php opens with plain fopen),
#     `..` is folded before a symlink is followed, so `l/../sub/x` with
#     `l -> /cli/sub` names /tmp/sub/x: the check in `run` finds the file, the
#     open does not.
out="$(docker run --rm -w /cli -v "$FIX:/cli:ro" "$IMAGE" oxphp run -d include_path=/cli/incpath:. sub/cwd.php 2>&1)"
[ "$out" = "/cli/sub/cwd.php|/cli" ] \
	&& ok "script not looked up in include_path" || bad "include_path lookup (got '$out')"
out="$(docker run --rm -w /tmp -v "$FIX:/cli:ro" "$IMAGE" oxphp run -d open_basedir=/tmp /cli/sub/cwd.php 2>&1)"; code=$?
{ [ "$out" = "/cli/sub/cwd.php|/tmp" ] && [ $code -eq 0 ]; } \
	&& ok "script outside open_basedir runs" || bad "open_basedir on the script (out='$out' code=$code)"
out="$(docker run --rm -w /tmp -v "$FIX:/cli:ro" "$IMAGE" sh -c 'ln -s /cli/sub/cwd.php link.php && oxphp run link.php' 2>&1)"
[ "$out" = "/cli/sub/cwd.php|/tmp" ] \
	&& ok "__FILE__ of a symlinked script is its target" || bad "symlink __FILE__ (got '$out')"
out="$(docker run --rm -w /tmp -v "$FIX:/cli:ro" "$IMAGE" sh -c 'ln -s /cli/sub l && oxphp run l/../sub/cwd.php' 2>&1)"; code=$?
{ [ "$out" = "oxphp: Could not open input file: l/../sub/cwd.php" ] && [ $code -eq 1 ]; } \
	&& ok ".. after a symlink is folded as under ZTS php" || bad "symlink/.. open (out='$out' code=$code)"

# 8k. A child started through PHP_BINARY with Symfony PhpSubprocess's `-n -c
#     <ini>` loads only that ini: its memory_limit, it is the loaded file, and
#     no conf.d file is scanned (the image has some, so `false` proves `-n`).
out="$(run php_cli_options.php subprocess 2>&1)"
ini="${out##*|ini=}"
[ "$out" = "out=99M|$ini|false|code=0|err=|ini=$ini" ] \
	&& ok "PHP_BINARY child: -n -c <ini>" || bad "-n -c (got '$out')"

# 8l. The bare binary started by `oxphp run` runs the script on its standard
#     input, as a bare php does (Symfony PhpProcess, PHPUnit process isolation),
#     with php-cli's names for it; `-d` before it applies.
out="$(run php_cli_options.php stdin 2>&1)"
[ "$out" = "out=42|cli|Standard input code|Standard input code||code=0|err=" ] \
	&& ok "PHP_BINARY child: script on stdin" || bad "stdin script (got '$out')"
out="$(run php_cli_options.php stdin-d 2>&1)"
[ "$out" = "out=88M|code=0|err=" ] \
	&& ok "PHP_BINARY child: -d + script on stdin" || bad "stdin script with -d (got '$out')"
#     A script on standard input that never compiles (the prepend throws) is
#     opened on a duplicate of fd 0, so fd 0 is still open in the shutdown
#     functions. php-cli closes it there; it opens the script on `stdin`, which
#     its own STDIN constant never closes, whereas the bootstrap's STDIN here
#     would close `stdin` a second time.
out="$(run php_cli_options.php stdin-prepend 2>&1)"
case "$out" in
	"out=stdin-open|code=255|err="*"Uncaught Error: prepend"*) ok "uncompiled stdin script keeps fd 0 open" ;;
	*) bad "stdin script after a throwing prepend (got '$out')" ;;
esac

# 8m. `-r <code>` runs the code with the arguments after it as $argv[1..], and
#     takes its exit code from exit() or from an uncaught exception (255).
out="$(run php_cli_options.php r 2>&1)"
[ "$out" = "out=42|Standard input code|x|Command line code|code=0|err=" ] \
	&& ok "PHP_BINARY child: -r" || bad "-r (got '$out')"
out="$(run php_cli_options.php r-exit 2>&1)"
[ "$out" = "out=|code=3|err=" ] && ok "-r exit(3) -> 3" || bad "-r exit (got '$out')"
out="$(run php_cli_options.php r-throw 2>&1)"
case "$out" in
	"out=|code=255|err="*boom*) ok "-r uncaught exception -> 255" ;;
	*) bad "-r throw (got '$out')" ;;
esac
out="$(run php_cli_options.php r-warn 2>&1)"
case "$out" in
	"out=|code=0|err="*"file_get_contents(/nonexistent): Failed to open stream"*) ok "-r warning names its function" ;;
	*) bad "-r warning (got '$out')" ;;
esac

# 8n. `-f <file>` names the script; the arguments after it are its $argv.
out="$(run php_cli_options.php f 2>&1)"
[ "$out" = "out=/cli/php_cli_options.php|child-argv|y|code=0|err=" ] \
	&& ok "PHP_BINARY child: -f <file>" || bad "-f (got '$out')"

# 8o. The bare binary not started by `oxphp run` itself is still the server,
#     even with the marker an `oxphp run` further up left in the environment and
#     a script on its standard input.
out="$(printf '<?php echo "STDIN-RAN\\n";' | docker run -i --rm -e OXPHP_RUN_PID=999999 \
	-e LISTEN_ADDR=127.0.0.1:18081 "$IMAGE" timeout 5 oxphp 2>&1)"
{ ! printf '%s' "$out" | grep -q STDIN-RAN && printf '%s' "$out" | grep -q 'Server listening'; } \
	&& ok "bare oxphp with a foreign OXPHP_RUN_PID serves" || bad "foreign marker (got '$(printf '%s' "$out" | head -c 300)')"

# 8p. A run that is PID 1 adopts an orphaned background `oxphp`, which then
#     passes the parent check; its standard input is /dev/null, so it serves.
out="$(run php_cli_options.php bg-serve 2>&1)"
[ "$out" = "pid=1|adopted|served" ] \
	&& ok "orphaned bare oxphp under a PID 1 run serves" || bad "orphan under PID 1 (got '$(printf '%s' "$out" | head -c 300)')"

# 9. regression: HTTP serve role still boots and answers, and still runs a
#    script in the script's own directory, as PHP-FPM does.
name="oxphp-serve-smoke-$$"
probe=/var/www/html/public/cwd_probe
docker run -d --rm --name "$name" -e LISTEN_ADDR=0.0.0.0:18080 -p 127.0.0.1:18080:18080 \
	-v "$FIX/sub:$probe:ro" "$IMAGE" >/dev/null 2>&1
served=""
for _ in $(seq 1 30); do
	if curl -fsS -o /dev/null "http://127.0.0.1:18080/" 2>/dev/null; then served=yes; break; fi
	sleep 0.5
done
body="$(curl -fsS "http://127.0.0.1:18080/cwd_probe/cwd.php" 2>&1)"
docker rm -f "$name" >/dev/null 2>&1
[ -n "$served" ] && ok "HTTP serve role boots + responds" || bad "HTTP serve smoke (no response)"
[ "$body" = "$probe/cwd.php|$probe" ] \
	&& ok "serve runs a script in its own directory" || bad "serve cwd (got '$body')"

echo "== $PASS passed, $FAIL failed =="
[ $FAIL -eq 0 ]
