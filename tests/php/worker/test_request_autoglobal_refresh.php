<?php
// $_REQUEST must describe the request in hand, not an earlier one served by the
// same worker.
//
// It is a lazy (JIT) auto-global: the engine materialises it when a script that
// mentions it is COMPILED, and OPcache re-fires it on a cached load only while
// the bit is still clear in its per-request mask. Worker mode runs the engine's
// request startup — and with it OPcache's mask reset — once per worker rather
// than once per request, so on a worker that does not rebuild $_REQUEST itself
// that re-fire happens exactly once in the worker's life. Every request after it
// reads the merged GET/POST/COOKIE of whichever request loaded the script first,
// i.e. another client's parameters.
//
// The check is deliberately narrow: $_REQUEST must agree with the $_GET of the
// request being served. It only bites on a SECOND sighting of this file by the
// same worker — the first one compiles it, and compiling materialises correctly —
// so the suites list it twice with different probe values. Under PHP_WORKERS=1
// both requests land on the same worker and the second is the stale case for
// certain; in a multi-worker pool it is a likelihood, and the response says which
// worker answered so a green run can be read for what it covered.
//
// OPcache is a precondition, in the direction that costs more: with it off,
// every request recompiles this file and compiling materialises $_REQUEST
// correctly, so the check passes on a build that has the defect. It is green
// either way — only under OPcache is that green worth anything.
//
// Deliberately written without test_helper.php: the tests under tests/php/worker
// pull it in with a bare `require`, so a worker serving two of them fatals on the
// class redeclare. Same constraint as test_request_state_after_reuse.php.

$worker = OxPHP\Server\Worker::current();
$fail = [];

$probe = $_GET['probe'] ?? null;
$request = $_REQUEST ?? null;

if (!is_array($request)) {
    $fail[] = '$_REQUEST is ' . get_debug_type($request) . ', expected array';
} elseif (($request['probe'] ?? null) !== $probe) {
    $fail[] = '$_REQUEST[probe] = ' . var_export($request['probe'] ?? null, true)
        . ', expected ' . var_export($probe, true)
        . " — \$_REQUEST is carrying an earlier request's parameters";
}

$where = json_encode([
    'worker_id'     => $worker->id(),
    'request_count' => $worker->requestCount(),
    'probe'         => $probe,
]);

if ($fail !== []) {
    http_response_code(500);
    echo "FAIL: " . implode('; ', $fail) . " ($where)\n";
    return;
}

echo "OK $where\n";
