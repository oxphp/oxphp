#!/usr/bin/env bash
# Runner-side assertion helpers.
# Sourced by run_profile.sh.

# run_php_test <base_url> <test_path> [curl_args...]
# Curls a PHP test, parses JSON response, outputs one JSONL line.
run_php_test() {
    local base_url="$1"
    local test_path="$2"
    shift 2

    # Parse optional URL suffix: "group/test >> /suffix"
    local url_suffix=""
    if [[ "$test_path" == *" >> "* ]]; then
        url_suffix="${test_path#* >> }"
        test_path="${test_path%% >> *}"
    fi

    local url="${base_url}/tests/${test_path}.php${url_suffix}"

    local response
    response=$(http_request "$url" "$@" 2>/dev/null) || response='{"status":0,"headers":{},"body":""}'

    local body
    body=$(printf '%s' "$response" | python3 -c "import sys,json; print(json.load(sys.stdin).get('body',''))" 2>/dev/null)

    # Check if body is valid test JSON
    if printf '%s' "$body" | python3 -c "import sys,json; d=json.load(sys.stdin); assert 'test' in d and 'pass' in d" 2>/dev/null; then
        printf '%s\n' "$body"
    else
        local test_name group status
        test_name=$(basename "$test_path")
        group=$(dirname "$test_path")
        status=$(printf '%s' "$response" | python3 -c "import sys,json; print(json.load(sys.stdin).get('status',0))" 2>/dev/null)
        printf '{"test":"%s","group":"%s","pass":false,"assertions":[],"error":"HTTP %s: non-JSON response","meta":{}}\n' \
            "$test_name" "$group" "$status"
    fi
}

# run_runner_test <base_url> <test_line>
# Runs a runner-side test with pipe-separated fields:
# path | method | extra_curl_args | expected_status | header_checks
run_runner_test() {
    local base_url="$1"
    local test_line="$2"

    IFS='|' read -ra fields <<< "$test_line"
    local test_path method curl_args expected_status header_checks
    test_path=$(echo "${fields[0]}" | xargs)
    method=$(echo "${fields[1]:-GET}" | xargs)
    # Do not pipe curl_args through xargs — it strips quotes and mangles
    # shell tokens like `-d ""` or `-H "X: y"`. Trim whitespace via parameter
    # expansion so embedded quotes survive for the eval below.
    curl_args="${fields[2]:-}"
    curl_args="${curl_args#"${curl_args%%[![:space:]]*}"}"
    curl_args="${curl_args%"${curl_args##*[![:space:]]}"}"
    expected_status=$(echo "${fields[3]:-200}" | xargs)
    header_checks=$(echo "${fields[4]:-}" | xargs)

    # Default empty expected_status to 200
    [ -z "$expected_status" ] && expected_status=200

    # Parse optional URL override: "group/test >> /custom/path"
    local url_override=""
    if [[ "$test_path" == *" >> "* ]]; then
        url_override="${test_path#* >> }"
        test_path="${test_path%% >> *}"
    fi

    local test_name group
    test_name=$(basename "$test_path")
    group=$(dirname "$test_path")

    # Determine URL — static file tests don't have .php
    local url
    if [ -n "$url_override" ]; then
        # Override starting with `?` is a query-string suffix on the default
        # PHP test URL; anything else (e.g. `/custom/path`) fully replaces it.
        if [[ "$url_override" == \?* ]]; then
            url="${base_url}/tests/${test_path}.php${url_override}"
        else
            url="${base_url}${url_override}"
        fi
    elif [[ "$test_path" == static_files/* ]]; then
        # Map static file tests to fixture paths
        case "$test_name" in
            test_css_content_type)  url="${base_url}/test_static/style.css" ;;
            test_js_content_type)   url="${base_url}/test_static/script.js" ;;
            test_png_content_type)  url="${base_url}/test_static/image.png" ;;
            test_etag_present)      url="${base_url}/test_static/style.css" ;;
            test_cache_control)     url="${base_url}/test_static/style.css" ;;
            test_last_modified)     url="${base_url}/test_static/style.css" ;;
            test_conditional_304)   url="${base_url}/test_static/style.css" ;;
            *)                      url="${base_url}/test_static/style.css" ;;
        esac
    else
        url="${base_url}/tests/${test_path}.php"
    fi

    # Build curl arguments
    local -a full_curl_args=(-X "$method")
    if [ -n "$curl_args" ]; then
        eval "full_curl_args+=($curl_args)"
    fi

    # For conditional 304 test, first get the ETag then resend with If-None-Match
    if [[ "$test_name" == "test_conditional_304" ]]; then
        local etag_response
        etag_response=$(http_request "$url" 2>/dev/null)
        local etag_value
        etag_value=$(printf '%s' "$etag_response" | python3 -c "import sys,json; print(json.load(sys.stdin).get('headers',{}).get('etag',''))" 2>/dev/null)
        if [ -n "$etag_value" ]; then
            full_curl_args+=(-H "If-None-Match: $etag_value")
        fi
    fi

    local response
    response=$(http_request "$url" "${full_curl_args[@]}" 2>/dev/null) || response='{"status":0,"headers":{},"body":""}'

    # Extract response parts
    local status headers_json body
    status=$(printf '%s' "$response" | python3 -c "import sys,json; print(json.load(sys.stdin).get('status',0))" 2>/dev/null)
    headers_json=$(printf '%s' "$response" | python3 -c "import sys,json; print(json.dumps(json.load(sys.stdin).get('headers',{})))" 2>/dev/null)

    # Build assertions
    local assertions_list="[]"
    local pass=true

    # Status check
    if [ "$status" = "$expected_status" ]; then
        assertions_list=$(python3 -c "
import json, sys
a = json.loads(sys.argv[1])
a.append({'name': 'HTTP status is $expected_status', 'pass': True})
print(json.dumps(a))" "$assertions_list")
    else
        assertions_list=$(python3 -c "
import json, sys
a = json.loads(sys.argv[1])
a.append({'name': 'HTTP status is $expected_status', 'pass': False, 'expected': '$expected_status', 'actual': '$status'})
print(json.dumps(a))" "$assertions_list")
        pass=false
    fi

    # Header checks
    if [ -n "$header_checks" ]; then
        IFS=',' read -ra checks <<< "$header_checks"
        for check in "${checks[@]}"; do
            check=$(echo "$check" | xargs)
            local hdr_name hdr_expect
            hdr_name=$(echo "$check" | cut -d: -f1 | tr '[:upper:]' '[:lower:]')
            hdr_expect=$(echo "$check" | cut -d: -f2-)

            local hdr_value
            hdr_value=$(printf '%s' "$headers_json" | python3 -c "
import sys,json
h=json.load(sys.stdin)
print(h.get('$hdr_name',''))" 2>/dev/null)

            if [ "$hdr_expect" = "exists" ]; then
                if [ -n "$hdr_value" ]; then
                    assertions_list=$(python3 -c "
import json, sys; a=json.loads(sys.argv[1]); a.append({'name':'header $hdr_name exists','pass':True}); print(json.dumps(a))" "$assertions_list")
                else
                    assertions_list=$(python3 -c "
import json, sys; a=json.loads(sys.argv[1]); a.append({'name':'header $hdr_name exists','pass':False,'expected':'exists','actual':'missing'}); print(json.dumps(a))" "$assertions_list")
                    pass=false
                fi
            elif [ "$hdr_expect" = "missing" ]; then
                if [ -z "$hdr_value" ]; then
                    assertions_list=$(python3 -c "
import json, sys; a=json.loads(sys.argv[1]); a.append({'name':'header $hdr_name missing','pass':True}); print(json.dumps(a))" "$assertions_list")
                else
                    assertions_list=$(python3 -c "
import json, sys; a=json.loads(sys.argv[1]); a.append({'name':'header $hdr_name missing','pass':False,'expected':'missing','actual':'$hdr_value'}); print(json.dumps(a))" "$assertions_list")
                    pass=false
                fi
            elif [[ "$hdr_expect" == \~* ]]; then
                # Prefix `~` enables substring match (useful when the server
                # appends `;charset=…` or other parameters to a content-type).
                local hdr_needle="${hdr_expect#\~}"
                if [[ "$hdr_value" == *"$hdr_needle"* ]]; then
                    assertions_list=$(python3 -c "
import json, sys; a=json.loads(sys.argv[1]); a.append({'name':'header $hdr_name contains $hdr_needle','pass':True}); print(json.dumps(a))" "$assertions_list")
                else
                    assertions_list=$(python3 -c "
import json, sys; a=json.loads(sys.argv[1]); a.append({'name':'header $hdr_name contains $hdr_needle','pass':False,'expected':'contains $hdr_needle','actual':'$hdr_value'}); print(json.dumps(a))" "$assertions_list")
                    pass=false
                fi
            else
                if [ "$hdr_value" = "$hdr_expect" ]; then
                    assertions_list=$(python3 -c "
import json, sys; a=json.loads(sys.argv[1]); a.append({'name':'header $hdr_name = $hdr_expect','pass':True}); print(json.dumps(a))" "$assertions_list")
                else
                    assertions_list=$(python3 -c "
import json, sys; a=json.loads(sys.argv[1]); a.append({'name':'header $hdr_name = $hdr_expect','pass':False,'expected':'$hdr_expect','actual':'$hdr_value'}); print(json.dumps(a))" "$assertions_list")
                    pass=false
                fi
            fi
        done
    fi

    # If the test also has a PHP file (not pure runner-side), check if response body has test JSON
    if [[ "$test_path" != static_files/* ]]; then
        local body
        body=$(printf '%s' "$response" | python3 -c "import sys,json; print(json.load(sys.stdin).get('body',''))" 2>/dev/null)
        if printf '%s' "$body" | python3 -c "import sys,json; d=json.load(sys.stdin); assert 'pass' in d" 2>/dev/null; then
            # Merge PHP assertions into our runner assertions
            local php_pass php_assertions
            php_pass=$(printf '%s' "$body" | python3 -c "import sys,json; print(json.load(sys.stdin).get('pass',False))" 2>/dev/null)
            php_assertions=$(printf '%s' "$body" | python3 -c "import sys,json; print(json.dumps(json.load(sys.stdin).get('assertions',[])))" 2>/dev/null)
            assertions_list=$(python3 -c "
import json, sys
a = json.loads(sys.argv[1])
b = json.loads(sys.argv[2])
a.extend(b)
print(json.dumps(a))" "$assertions_list" "$php_assertions")
            if [ "$php_pass" = "False" ]; then
                pass=false
            fi
        fi
    fi

    printf '{"test":"%s","group":"%s","pass":%s,"assertions":%s,"error":null,"meta":{}}\n' \
        "$test_name" "$group" "$pass" "$assertions_list"
}

is_runner_test() {
    [[ "$1" == *"|"* ]]
}
