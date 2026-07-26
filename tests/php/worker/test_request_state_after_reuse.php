<?php
// A worker serves its first request on a fresh request fiber and every later
// one on a recycled fiber. Both must start from the state the per-request reset
// just built — nothing may be re-installed from the recycled fiber's saved
// snapshot, which describes a request that SUSPENDED and nothing else.
//
// Each check below reads a different piece of that state, and each one used to
// hold on a worker's first request only, because the recycled path restored a
// zeroed snapshot: SG(sapi_headers).http_response_code came back as 0,
// SG(sapi_headers).headers became a list with zero element size whose entries
// held uninitialized heap, and PG(http_globals) went IS_UNDEF — which the
// $_REQUEST auto-global callback dereferences without a type check.
//
// Deliberately written without test_helper.php: the tests under tests/php/worker
// pull it in with a bare `require`, so a worker serving two of them fatals on the
// class redeclare. (tests/php/hooks/* use `require_once` and do not have that
// problem — the blocker is the bare require, not the helper itself.) This test
// has to survive being served on any request index in either profile.
//
// Note on how a regression surfaces: the $_REQUEST check faults in the engine
// rather than returning bad data, so a broken build dies with SIGSEGV instead
// of printing FAIL — this test and every later one in the profile then report
// "request failed". The checks are still listed separately so a partial
// regression that clobbers only one field reports precisely.

$worker = OxPHP\Server\Worker::current();
$fail = [];

// Nothing below exercises the recycled-fiber path on a worker's first request,
// and a silent pass would look like coverage. Where reuse is guaranteed (a
// single-PHP-worker profile, this test listed second) the caller passes
// strict=1 and a first request is a failure; where the pool size makes it a
// likelihood, the run stays green and says so in the body instead.
//
// requestCount() is the only probe PHP has here, and it is a NECESSARY, not a
// sufficient, condition for reuse: a request that arrives while a sibling is
// suspended gets a fiber of its own whatever the worker's request index. In the
// strict case reuse is guaranteed by the suite's placement (single worker, a
// preceding request that did not suspend), so the count only rules out the one
// case placement cannot — being first. Outside it the reported flag says
// "probably", because from PHP it cannot say more.
$count = $worker->requestCount();
$fresh = $count < 2;
$strict = ($_GET['strict'] ?? null) === '1';
if ($fresh && $strict) {
    $fail[] = 'served on a fresh fiber (requestCount=' . $count
        . ') — the recycled-fiber path was not exercised';
}

// SG(sapi_headers).http_response_code — 200 until the script changes it.
// http_response_code() returns false when the field reads back as 0.
$status = http_response_code();
if ($status !== 200) {
    $fail[] = 'http_response_code() = ' . var_export($status, true) . ', expected 200';
}

// SG(sapi_headers).headers — the real list, with the right element size, so
// headers_list() reports what was set and a replace collapses to one entry.
// The replace is the step that walks the list element by element
// (sapi_remove_header), i.e. the one that read uninitialized heap.
header('X-Reuse-One: 1');
header('X-Reuse-Two: 2');
$listed = headers_list();
foreach (['X-Reuse-One: 1', 'X-Reuse-Two: 2'] as $expected) {
    if (!in_array($expected, $listed, true)) {
        $fail[] = "headers_list() missing '$expected': " . json_encode($listed);
    }
}

header('X-Reuse-Two: replaced');
$two = array_values(array_filter(
    headers_list(),
    static fn(string $h): bool => str_starts_with($h, 'X-Reuse-Two:')
));
if ($two !== ['X-Reuse-Two: replaced']) {
    $fail[] = 'replaced header did not collapse to one entry: ' . json_encode($two);
}

// PG(http_globals) — $_REQUEST is a lazy auto-global built by merging
// PG(http_globals)[GET/POST/COOKIE], so it is the one observable that reads
// those slots directly.
if (($_GET['probe'] ?? null) !== '42') {
    $fail[] = '$_GET[probe] = ' . var_export($_GET['probe'] ?? null, true);
}
if (($_REQUEST['probe'] ?? null) !== '42') {
    $fail[] = '$_REQUEST[probe] = ' . var_export($_REQUEST['probe'] ?? null, true);
}

$where = json_encode([
    'worker_id'        => $worker->id(),
    'request_count'    => $count,
    'probably_reused'  => !$fresh,
]);

if ($fail !== []) {
    http_response_code(500);
    echo "FAIL: " . implode('; ', $fail) . " ($where)\n";
    return;
}

echo "OK $where\n";
