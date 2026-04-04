#!/usr/bin/env bash
# OxPHP PHP Test Suite Runner
# Usage: ./tests/run_all.sh [options]
#   --profile=NAME     Run only this profile
#   --suite=NAME       Run only this suite
#   --test=GROUP/NAME  Run only this test
#   --verbose          Show all assertions, not just failures
#   --no-build         Skip Docker image build
#   --output=FORMAT    Output format: terminal (default), json, html
#   --parallel=N       Max parallel profiles (default: 4)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "${SCRIPT_DIR}/lib/common.sh"
source "${SCRIPT_DIR}/lib/reporter.sh"

# ── Parse arguments ──────────────────────────────────────────
FILTER_PROFILE=""
FILTER_SUITE=""
FILTER_TEST=""
VERBOSE=""
NO_BUILD=""
OUTPUT="terminal"
PARALLEL=4

for arg in "$@"; do
    case "$arg" in
        --profile=*)    FILTER_PROFILE="${arg#*=}" ;;
        --suite=*)      FILTER_SUITE="${arg#*=}" ;;
        --test=*)       FILTER_TEST="${arg#*=}" ;;
        --verbose)      VERBOSE="--verbose" ;;
        --no-build)     NO_BUILD="--no-build" ;;
        --output=*)     OUTPUT="${arg#*=}" ;;
        --parallel=*)   PARALLEL="${arg#*=}" ;;
        --help|-h)
            sed -n '2,10p' "$0"
            exit 0 ;;
        *) log_error "Unknown argument: $arg"; exit 1 ;;
    esac
done

# ── Determine profiles to run ────────────────────────────────
profiles=()
if [ -n "$FILTER_PROFILE" ]; then
    profiles=("$FILTER_PROFILE")
elif [ -n "$FILTER_SUITE" ]; then
    suite_file="${TESTS_DIR}/suites/${FILTER_SUITE}.txt"
    if [ ! -f "$suite_file" ]; then
        log_error "Suite not found: $FILTER_SUITE"
        exit 1
    fi
    profiles=($(get_suite_profile "$suite_file"))
elif [ -n "$FILTER_TEST" ]; then
    for f in "${TESTS_DIR}/suites/"*.txt; do
        if grep -q "^${FILTER_TEST}\$" "$f" 2>/dev/null || \
           grep -q "^${FILTER_TEST} " "$f" 2>/dev/null || \
           grep -q "^${FILTER_TEST}|" "$f" 2>/dev/null; then
            profiles+=($(get_suite_profile "$f"))
            break
        fi
    done
    if [ ${#profiles[@]} -eq 0 ]; then
        log_error "Test not found in any suite: $FILTER_TEST"
        exit 1
    fi
else
    mapfile -t profiles < <(list_profiles)
fi

# ── Generate TLS certs if needed ─────────────────────────────
for p in "${profiles[@]}"; do
    [ "$p" = "tls" ] && generate_certs
done

# ── Setup ────────────────────────────────────────────────────
RESULTS_DIR="${TESTS_DIR}/reports"
mkdir -p "$RESULTS_DIR"
JSONL_FILE=$(mktemp)
START_TIME=$(date +%s)

log_info "Profiles: ${profiles[*]}"
log_info "Output: $OUTPUT"

# ── Build (once — all profiles share the same image) ─────────
if [ -z "$NO_BUILD" ]; then
    log_info "Building Docker image..."
    eval "$(compose_cmd "${profiles[0]}") build --quiet" 2>/dev/null || true
    log_ok "Build complete"
fi

# ── Cleanup on exit ──────────────────────────────────────────
cleanup() {
    log_info "Cleaning up..."
    for p in "${profiles[@]}"; do
        stop_profile "$p" 2>/dev/null || true
    done
    rm -f "$JSONL_FILE"
}
trap cleanup EXIT

# ── Run profiles sequentially ────────────────────────────────
for profile in "${profiles[@]}"; do
    log_info "━━━ Profile: $profile ━━━"
    start_profile "$profile"
    if ! wait_healthy "$profile" 60; then
        log_error "Profile $profile failed to start, skipping"
        stop_profile "$profile"
        continue
    fi

    local_base_url="http://oxphp-${profile}"
    if [ "$profile" = "tls" ]; then
        local_base_url="https://oxphp-${profile}"
    fi

    "${SCRIPT_DIR}/run_profile.sh" "$profile" "$local_base_url" "$VERBOSE" >> "$JSONL_FILE" 2>/dev/null || true

    stop_profile "$profile"
done

END_TIME=$(date +%s)
DURATION=$((END_TIME - START_TIME))

# ── Generate reports ─────────────────────────────────────────
report_json "$JSONL_FILE" "${RESULTS_DIR}/results.json" "$DURATION"
report_html "$JSONL_FILE" "${RESULTS_DIR}/report.html" "$DURATION"

case "$OUTPUT" in
    terminal)
        report_terminal "$JSONL_FILE" "$VERBOSE"
        exit_code=$?
        log_info "Reports saved: ${RESULTS_DIR}/results.json, ${RESULTS_DIR}/report.html"
        exit $exit_code
        ;;
    json)
        cat "${RESULTS_DIR}/results.json"
        ;;
    html)
        cat "${RESULTS_DIR}/report.html"
        ;;
esac
