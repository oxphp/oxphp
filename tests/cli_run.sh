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
