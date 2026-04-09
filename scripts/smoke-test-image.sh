#!/usr/bin/env bash
# scripts/smoke-test-image.sh
#
# Smoke-test a built oxphp Docker image. Run per-cell in CI before pushing,
# and usable locally against `docker compose build` output.
#
# Usage:
#   scripts/smoke-test-image.sh <image-ref> [expected-php-version]
#
# Arguments:
#   image-ref             Docker image reference (e.g., oxphp:ci-amd64)
#   expected-php-version  Optional. Exact PHP version string (e.g., "8.4.15").
#                         If omitted, only checks that PHP starts and reports *some*
#                         ZTS version. If provided, strict equality is required.
#
# Exits non-zero on first failing check.

set -euo pipefail

IMG="${1:?usage: $0 <image-ref> [expected-php-version]}"
EXPECTED_PHP="${2:-}"

FAIL=0
fail() { echo "FAIL: $1" >&2; FAIL=1; }
ok() { echo "OK:   $1"; }

# Check 1: PHP CLI exists and is ZTS
if docker run --rm "$IMG" php -v 2>/dev/null | grep -q "ZTS"; then
    ok "check #1: php CLI exists and reports ZTS"
else
    fail "check #1: php CLI missing or not ZTS"
fi

# Check 2: PHP version matches expectation (if provided)
ACTUAL_PHP=$(docker run --rm "$IMG" php -r 'echo PHP_VERSION;' 2>/dev/null || echo "MISSING")
if [[ -n "$EXPECTED_PHP" ]]; then
    if [[ "$ACTUAL_PHP" == "$EXPECTED_PHP" ]]; then
        ok "check #2: PHP version is exactly $EXPECTED_PHP"
    else
        fail "check #2: expected PHP $EXPECTED_PHP, got $ACTUAL_PHP"
    fi
else
    if [[ "$ACTUAL_PHP" != "MISSING" && "$ACTUAL_PHP" =~ ^[0-9]+\.[0-9]+\.[0-9]+ ]]; then
        ok "check #2: PHP version reports as $ACTUAL_PHP (no expectation set)"
    else
        fail "check #2: could not read PHP version, got '$ACTUAL_PHP'"
    fi
fi

# Check 3: oxphp SAPI extension loads (actual extension name is oxphp_sapi,
# per PHP_OXPHP_SAPI_EXTNAME in ext/php_oxphp_sapi.h)
if docker run --rm "$IMG" php -m 2>/dev/null | grep -qx "oxphp_sapi"; then
    ok "check #3: oxphp_sapi extension is loaded"
else
    fail "check #3: oxphp_sapi extension not found in php -m"
fi

# Check 4: oxphp binary serves HTTP and executes a PHP-rendered page
CID=""
cleanup() { [[ -n "$CID" ]] && docker rm -f "$CID" >/dev/null 2>&1 || true; }
trap cleanup EXIT INT TERM

CID=$(docker run -d --rm -p 0:80 "$IMG" || true)
if [[ -z "$CID" ]]; then
    fail "check #4: could not start container"
else
    HOST_PORT=$(docker port "$CID" 80 2>/dev/null | head -1 | awk -F: '{print $NF}')
    if [[ -z "$HOST_PORT" ]]; then
        fail "check #4: could not resolve host port mapping"
    else
        # Poll / for up to 10s. We fetch the body and store it to avoid a
        # second network call after the poll succeeds.
        HEALTH_OK=0
        BODY=""
        for ((i=0; i<50; i++)); do
            if BODY=$(curl -sf "http://localhost:$HOST_PORT/" 2>/dev/null); then
                HEALTH_OK=1
                break
            fi
            sleep 0.2
        done
        if [[ "$HEALTH_OK" -eq 1 ]]; then
            if [[ -n "$BODY" ]]; then
                ok "check #4: oxphp serves HTTP on port 80 and returns a non-empty body"
            else
                fail "check #4: oxphp HTTP responded but body is empty"
            fi
        else
            fail "check #4: oxphp did not respond on port 80 within 10s"
            docker logs "$CID" 2>&1 | tail -20 >&2 || true
        fi
    fi
fi

# Check 5: docker-php-ext-install is present (enables user RUN docker-php-ext-install ...)
if docker run --rm "$IMG" which docker-php-ext-install >/dev/null 2>&1; then
    ok "check #5: docker-php-ext-install is available for user Dockerfiles"
else
    fail "check #5: docker-php-ext-install is missing — users cannot install PHP extensions"
fi

if [[ "$FAIL" -ne 0 ]]; then
    echo "SMOKE TEST FAILED" >&2
    exit 1
fi
echo "SMOKE TEST PASSED"
