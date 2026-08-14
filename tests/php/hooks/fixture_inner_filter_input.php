<?php

declare(strict_types=1);

// Inner self-request for hooks/test_filter_input_not_shared ?mode=suspend. It
// can only be served while the outer request's fiber is parked in a hooked
// sleep, which is the whole point: what ext/filter reads for INPUT_GET,
// INPUT_POST and INPUT_COOKIE is three arrays per thread rather than per
// request, so this request is the one that reads them while another request is
// live and holding its own input in them.
//
// Two claims from this side: this request's own query is readable, and nothing
// of the parked request's query or cookie — nor of the seed request's body,
// which the same worker parsed earlier in the profile — is.
//
// Must not suspend — no await, no sleep, no socket read. This request exists to
// run to completion inside the outer one's window.
//
// Echo-style on purpose — the outer test asserts on this body, not on JSON.

$fail = [];

$own = filter_input(INPUT_GET, 'oxfilter_inner');
if ($own !== 'inner-query-value') {
    $fail[] = 'filter_input(INPUT_GET, oxfilter_inner) = ' . var_export($own, true)
        . ', expected this request\'s own query value';
}

$outerGet = filter_input(INPUT_GET, 'oxfilter_token');
if ($outerGet !== null) {
    $fail[] = 'filter_input(INPUT_GET, oxfilter_token) = ' . var_export($outerGet, true)
        . ', expected null — that is the parked request\'s query';
}

if (filter_has_var(INPUT_COOKIE, 'oxfilter_sid') !== false) {
    $fail[] = 'filter_has_var(INPUT_COOKIE, oxfilter_sid) = true, and this request sent no cookie';
}

$outerCookie = filter_input(INPUT_COOKIE, 'oxfilter_sid');
if ($outerCookie !== null) {
    $fail[] = 'filter_input(INPUT_COOKIE, oxfilter_sid) = ' . var_export($outerCookie, true)
        . ', expected null — that is another request\'s session id';
}

$seedPost = filter_input(INPUT_POST, 'oxfilter_pw');
if ($seedPost !== null) {
    $fail[] = 'filter_input(INPUT_POST, oxfilter_pw) = ' . var_export($seedPost, true)
        . ', expected null — this request has no body, that is the seed request\'s';
}

$meta = json_encode(['tag' => $_GET['tag'] ?? '?']);

if ($fail !== []) {
    http_response_code(500);
    echo "INNER-FAIL $meta: " . implode('; ', $fail) . "\n";
    return;
}

echo "INNER-OK $meta\n";
