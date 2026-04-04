#!/usr/bin/env bash
# Common functions for the OxPHP PHP test runner.
# Sourced by run_all.sh and run_profile.sh.

set -euo pipefail

TESTS_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROJECT_DIR="$(cd "$TESTS_DIR/.." && pwd)"

# ── Colors ───────────────────────────────────────────────────
if [ -t 1 ]; then
    RED='\033[0;31m'
    GREEN='\033[0;32m'
    YELLOW='\033[0;33m'
    BLUE='\033[0;34m'
    BOLD='\033[1m'
    RESET='\033[0m'
else
    RED='' GREEN='' YELLOW='' BLUE='' BOLD='' RESET=''
fi

# ── Logging ──────────────────────────────────────────────────
log_info()  { echo -e "${BLUE}[INFO]${RESET}  $*" >&2; }
log_ok()    { echo -e "${GREEN}[OK]${RESET}    $*" >&2; }
log_warn()  { echo -e "${YELLOW}[WARN]${RESET}  $*" >&2; }
log_error() { echo -e "${RED}[ERROR]${RESET} $*" >&2; }

# ── Docker helpers ───────────────────────────────────────────

compose_cmd() {
    local profile="$1"
    echo "docker compose -f ${TESTS_DIR}/compose.yml -f ${TESTS_DIR}/compose.${profile}.yml"
}

build_profile() {
    local profile="$1"
    local no_build="${2:-}"
    if [ "$no_build" = "--no-build" ]; then
        log_info "Skipping build for profile: $profile"
        return 0
    fi
    log_info "Building profile: $profile"
    eval "$(compose_cmd "$profile") build --quiet" 2>/dev/null
}

start_profile() {
    local profile="$1"
    local service="oxphp-${profile}"
    log_info "Starting $service"
    eval "$(compose_cmd "$profile") up -d $service" 2>/dev/null
}

stop_profile() {
    local profile="$1"
    log_info "Stopping profile: $profile"
    eval "$(compose_cmd "$profile") down --remove-orphans" 2>/dev/null || true
}

wait_healthy() {
    local profile="$1"
    local timeout="${2:-60}"
    local service="oxphp-${profile}"
    local elapsed=0

    log_info "Waiting for $service to be healthy (timeout: ${timeout}s)"
    while [ $elapsed -lt "$timeout" ]; do
        local status
        status=$(eval "$(compose_cmd "$profile") ps --format '{{.Health}}' $service" 2>/dev/null | head -1)
        if [ "$status" = "healthy" ]; then
            log_ok "$service is healthy (${elapsed}s)"
            return 0
        fi
        sleep 1
        elapsed=$((elapsed + 1))
    done
    log_error "$service did not become healthy within ${timeout}s"
    return 1
}

# ── TLS certificate generation ───────────────────────────────

generate_certs() {
    local certs_dir="${TESTS_DIR}/fixtures/certs"
    if [ -f "$certs_dir/cert.pem" ] && [ -f "$certs_dir/key.pem" ]; then
        return 0
    fi
    log_info "Generating self-signed TLS certificates"
    mkdir -p "$certs_dir"
    openssl req -x509 -newkey rsa:2048 -nodes \
        -keyout "$certs_dir/key.pem" \
        -out "$certs_dir/cert.pem" \
        -days 1 \
        -subj "/CN=oxphp-test" \
        2>/dev/null
}

# ── HTTP helpers ─────────────────────────────────────────────

# http_request <url> [curl_args...]
# Returns JSON: {"status": N, "headers": "raw_headers", "body": "response_body"}
http_request() {
    local url="$1"
    shift
    local tmp_headers
    tmp_headers=$(mktemp)
    local tmp_body
    tmp_body=$(mktemp)

    local status
    status=$(curl -s -o "$tmp_body" -D "$tmp_headers" -w '%{http_code}' "$@" "$url" 2>/dev/null || echo "000")

    local body
    body=$(cat "$tmp_body")
    local headers_raw
    headers_raw=$(cat "$tmp_headers")

    rm -f "$tmp_headers" "$tmp_body"

    # Use python3 to safely construct JSON
    python3 -c "
import json, sys
status = int(sys.argv[1])
body = sys.argv[2]
headers_raw = sys.argv[3]
headers = {}
for line in headers_raw.split('\n'):
    line = line.strip()
    if ': ' in line:
        key, _, value = line.partition(': ')
        headers[key.lower()] = value
print(json.dumps({'status': status, 'headers': headers, 'body': body}, ensure_ascii=False))
" "$status" "$body" "$headers_raw"
}

# ── Suite parsing ────────────────────────────────────────────

get_suite_profile() {
    local suite_file="$1"
    grep -m1 '^# profile:' "$suite_file" | sed 's/^# profile:[[:space:]]*//' || echo "default"
}

get_suite_tests() {
    local suite_file="$1"
    grep -v '^#' "$suite_file" | grep -v '^[[:space:]]*$' || true
}

list_profiles() {
    for f in "${TESTS_DIR}"/compose.*.yml; do
        [ -f "$f" ] || continue
        basename "$f" | sed 's/^compose\.//;s/\.yml$//'
    done
}

list_suites() {
    local filter_profile="${1:-}"
    for f in "${TESTS_DIR}"/suites/*.txt; do
        [ -f "$f" ] || continue
        if [ -n "$filter_profile" ]; then
            local profile
            profile=$(get_suite_profile "$f")
            [ "$profile" = "$filter_profile" ] || continue
        fi
        echo "$f"
    done
}
