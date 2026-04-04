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
    while IFS= read -r test_line; do
        [ -z "$test_line" ] && continue

        local_result=""
        if is_runner_test "$test_line"; then
            local_result=$(run_runner_test "$BASE_URL" "$test_line" 2>/dev/null) || local_result=""
        else
            test_path=$(echo "$test_line" | xargs)
            local_result=$(run_php_test "$BASE_URL" "$test_path" 2>/dev/null) || local_result=""
        fi

        if [ -z "$local_result" ]; then
            local clean_line="${test_line%% >> *}"
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
