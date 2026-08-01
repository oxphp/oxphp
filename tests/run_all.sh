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
    profiles=()
    while IFS= read -r p; do profiles+=("$p"); done < <(list_profiles)
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

# ── Build (each profile owns a tests-oxphp-<profile> image) ──
# Layer cache makes the second-onward builds near-instant; without
# this loop, profiles whose image already exists locally reuse a
# stale build (e.g. from a prior commit) and miss new symbols.
if [ -z "$NO_BUILD" ]; then
    log_info "Building Docker images for ${#profiles[@]} profile(s)..."
    for p in "${profiles[@]}"; do
        eval "$(compose_cmd "$p") build --quiet" 2>/dev/null || true
    done
    log_ok "Build complete"
fi

# ── Cleanup on exit ──────────────────────────────────────────
cleanup() {
    log_info "Cleaning up..."
    for p in "${profiles[@]}"; do
        stop_profile "$p" 2>/dev/null || true
    done
    rm -f "$JSONL_FILE"
    cleanup_certs 2>/dev/null || true
}
trap cleanup EXIT

# ── Run profiles sequentially ────────────────────────────────
for profile in "${profiles[@]}"; do
    log_info "━━━ Profile: $profile ━━━"
    # A profile that cannot even be brought up will never turn healthy, and
    # waiting out the health timeout to discover that costs a minute each time.
    # Profiles layered on the dev image (hooksdb) land here on a machine that
    # has not built it.
    if ! start_profile "$profile"; then
        log_error "Profile $profile could not be started (check the prerequisites in its compose file), skipping"
        continue
    fi
    if ! wait_healthy "$profile" 60; then
        log_error "Profile $profile failed to start, skipping"
        stop_profile "$profile"
        continue
    fi

    mapped_port=$(get_mapped_port "$profile")
    if [ -z "$mapped_port" ]; then
        log_error "Could not get mapped port for $profile, skipping"
        stop_profile "$profile"
        continue
    fi

    local_base_url="http://127.0.0.1:${mapped_port}"
    if [ "$profile" = "tls" ]; then
        local_base_url="https://127.0.0.1:${mapped_port}"
    fi
    log_info "Base URL: $local_base_url"

    "${SCRIPT_DIR}/run_profile.sh" "$profile" "$local_base_url" "$VERBOSE" >> "$JSONL_FILE" 2>/dev/null || true

    stop_profile "$profile"
done

# ── Standalone concurrency checks ────────────────────────────
# Admission control can only be observed with concurrent requests: a 529 is the
# answer to a *different* request than the one under test, and the suite runner
# issues one request at a time. These ride on the overflow profile's image.
if [[ " ${profiles[*]} " == *" overflow "* ]] && [ -z "$FILTER_SUITE" ] && [ -z "$FILTER_TEST" ]; then
    overload_image="tests-oxphp-overflow:latest"
    if docker image inspect "$overload_image" >/dev/null 2>&1; then
        log_info "━━━ Standalone: queue admission control ━━━"
        emitted_before=$(wc -l < "$JSONL_FILE")
        # A failing check emits its own JSONL and turns the run red on its own,
        # so a non-zero exit is expected and not itself news. What has to be
        # caught is a script that dies before emitting anything: the report is
        # assembled from JSONL, so with `|| true` alone that leaves a green run
        # with no admission coverage in it at all.
        "${SCRIPT_DIR}/overload_529.sh" "$overload_image" --jsonl >> "$JSONL_FILE" || true
        if [ "$(wc -l < "$JSONL_FILE")" -eq "$emitted_before" ]; then
            log_error "Admission checks produced no results — recording as a failure"
            printf '%s\n' '{"test": "admission checks produced results", "group": "admission", "pass": false, "assertions": [], "error": "overload_529.sh emitted nothing", "meta": {}, "profile": "overflow"}' >> "$JSONL_FILE"
        fi
    else
        log_error "Skipping admission checks: image $overload_image not found (build the overflow profile first)"
    fi
fi

# ── Standalone worker-lifetime checks ────────────────────────
# None of these can be asserted from inside a request. A request cannot observe
# its own teardown, what a request leaves behind only shows up as the whole
# worker's allocator across hundreds of them, and a response is not something
# the request that produced it can read back. All three drive the fibers profile
# themselves, from outside, after its suite has finished.
if [[ " ${profiles[*]} " == *" fibers "* ]] && [ -z "$FILTER_SUITE" ] && [ -z "$FILTER_TEST" ]; then
    for check in verify_teardown_unwind verify_worker_does_not_grow verify_response_headers; do
        log_info "━━━ Standalone: ${check#verify_} ━━━"
        emitted_before=$(wc -l < "$JSONL_FILE")
        # Same reasoning as the admission checks above: a failing check turns the
        # run red through its own JSONL, so what needs catching here is a script
        # that dies before emitting anything at all.
        "${SCRIPT_DIR}/scripts/${check}.sh" --jsonl >> "$JSONL_FILE" || true
        if [ "$(wc -l < "$JSONL_FILE")" -eq "$emitted_before" ]; then
            log_error "${check}.sh produced no results — recording as a failure"
            printf '{"test": "%s produced results", "group": "fibers", "pass": false, "assertions": [], "error": "%s.sh emitted nothing", "meta": {}, "profile": "fibers"}\n' \
                "$check" "$check" >> "$JSONL_FILE"
        fi
    done
fi

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
