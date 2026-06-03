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

# 5b. CLI ini defaults from the ini_entries blob apply (max_execution_time=0,
#     register_argc_argv=1) — guards against the sapi_startup ini_entries drop.
out="$(run cliini.php)"
met="$(printf '%s' "$out" | cut -d'|' -f1)"
argc="$(printf '%s' "$out" | cut -d'|' -f2)"
{ [ "$met" = "0" ] && [ "$argc" != "NO-ARGC" ]; } \
	&& ok "CLI ini defaults (max_execution_time=0, argc)" || bad "cli ini defaults (got '$out')"

# 5c. -d works for a PHP_INI_PERDIR directive (config stage, not runtime alter)
out="$(docker run --rm -v "$FIX:/cli:ro" "$IMAGE" oxphp run -d max_input_vars=1234 /cli/inivar.php)"
[ "$out" = "1234" ] && ok "-d for PHP_INI_PERDIR (max_input_vars)" || bad "-d PERDIR (got '$out')"

# 6. async engine available under one-shot
out="$(run async.php)"
printf '%s' "$out" | grep -q "async-ok" && ok "async (oxphp_sleep)" || bad "async (got '$out')"

# 7. ox_shared classes registered
out="$(run shared.php)"
printf '%s' "$out" | grep -q "shared-ok" && ok "ox_shared classes" || bad "shared (got '$out')"

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

# 9. regression: HTTP serve role still boots and answers
name="oxphp-serve-smoke-$$"
docker run -d --rm --name "$name" -e LISTEN_ADDR=0.0.0.0:18080 -p 127.0.0.1:18080:18080 "$IMAGE" >/dev/null 2>&1
served=""
for _ in $(seq 1 30); do
	if curl -fsS -o /dev/null "http://127.0.0.1:18080/" 2>/dev/null; then served=yes; break; fi
	sleep 0.5
done
docker rm -f "$name" >/dev/null 2>&1
[ -n "$served" ] && ok "HTTP serve role boots + responds" || bad "HTTP serve smoke (no response)"

echo "== $PASS passed, $FAIL failed =="
[ $FAIL -eq 0 ]
