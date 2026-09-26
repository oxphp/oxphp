#!/usr/bin/env bash
# Runs all test suites for a single profile.
# Usage: run_profile.sh <profile> <base_url> [--verbose]
# Outputs: JSONL to stdout (one line per test).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
source "${SCRIPT_DIR}/lib/common.sh"
source "${SCRIPT_DIR}/lib/assertions.sh"

PROFILE="$1"
BASE_URL="$2"
VERBOSE="${3:-}"

# Find all suites for this profile
suite_files=()
for f in "${TESTS_DIR}/suites/"*.txt; do
    [ -f "$f" ] || continue
    p=$(get_suite_profile "$f")
    [ "$p" = "$PROFILE" ] || continue
    suite_files+=("$f")
done

if [ ${#suite_files[@]} -eq 0 ]; then
    log_warn "No suites found for profile: $PROFILE"
    exit 0
fi

for suite_file in "${suite_files[@]}"; do
    # A `log ~` line searches only what the server wrote since the first request
    # of the latest block that sent any — a block being a run of lines between
    # blank ones. `log ~` lines send nothing, so they read the window of the
    # requests above them wherever they sit, and consecutive ones share it. A
    # block with no `log ~` line of its own still opens a window, so what it
    # logs cannot answer for a later block.
    log_from=0
    block_head=""
    # Taking a mark reads the whole log, so a suite with nothing to assert on it
    # never takes one.
    marks_log=false
    grep -q '^log ~' "$suite_file" && marks_log=true
    new_block=$marks_log
    while IFS= read -r test_line; do
        if [[ "$test_line" =~ ^[[:space:]]*$ ]]; then
            new_block=$marks_log
            continue
        fi

        if ! is_log_assertion "$test_line" && [ "$new_block" = true ]; then
            # A failed read widens the window to the whole log rather than
            # ending this script, which would drop the rest of the profile's
            # results without a failure being recorded for any of them.
            log_from=$(server_log "$PROFILE" | wc -l | tr -d ' ') || log_from=0
            # The path alone, without a `>> /override` or the `|` fields.
            block_head="${test_line%%|*}"
            block_head="${block_head%% >> *}"
            block_head=$(echo "$block_head" | xargs)
            new_block=false
        fi

        local_result=""
        if is_log_assertion "$test_line"; then
            local_result=$(run_log_assertion "$PROFILE" "${test_line#log ~}" "$log_from" "$block_head" 2>/dev/null) || local_result=""
        elif is_runner_test "$test_line"; then
            local_result=$(run_runner_test "$BASE_URL" "$test_line" 2>/dev/null) || local_result=""
        else
            test_path=$(echo "$test_line" | xargs)
            local_result=$(run_php_test "$BASE_URL" "$test_path" 2>/dev/null) || local_result=""
        fi

        if [ -z "$local_result" ]; then
            # NB: this block runs at script scope, not inside a function,
            # so `local` is not valid here.
            clean_line="${test_line%% >> *}"
            test_name=$(basename "$(echo "$clean_line" | cut -d'|' -f1 | xargs)")
            group=$(dirname "$(echo "$clean_line" | cut -d'|' -f1 | xargs)")
            local_result=$(printf '{"test":"%s","group":"%s","pass":false,"assertions":[],"error":"request failed","meta":{}}' "$test_name" "$group")
        fi

        # Inject profile
        local_result=$(printf '%s' "$local_result" | python3 -c "
import sys,json
d=json.load(sys.stdin)
d['profile']='$PROFILE'
print(json.dumps(d,ensure_ascii=False))" 2>/dev/null) || true

        printf '%s\n' "$local_result"
    done < <(get_suite_tests "$suite_file")
done
