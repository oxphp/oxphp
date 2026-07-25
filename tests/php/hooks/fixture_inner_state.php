<?php

declare(strict_types=1);

// Inner self-request for hooks/test_tick_path_fiber_reuse. It can only be served
// while the outer request's fiber is suspended, so the worker dispatches it from
// the event-loop tick rather than the fast path. Two of these run inside the same
// suspended window, and the second one runs on the fiber the first returned to
// the free list — the tick-path counterpart of the fast-path coverage in
// tests/php/worker/test_request_state_after_reuse.php.
//
// Checks the SAPI state a recycled fiber used to lose when a new request was
// handed to it as a resume: the status default, and the header list across a
// replace (the step that walks the list element by element, i.e. the one that
// read uninitialized heap and faulted).
//
// Echo-style on purpose — the outer test asserts on this body, not on JSON. Must
// not mention $_REQUEST; see the note at the top of tests/suites/hooks.txt.

$worker = OxPHP\Server\Worker::current();
$fail = [];

$status = http_response_code();
if ($status !== 200) {
    $fail[] = 'http_response_code() = ' . var_export($status, true) . ', expected 200';
}

header('X-Inner-One: 1');
header('X-Inner-Two: 2');
$listed = headers_list();
foreach (['X-Inner-One: 1', 'X-Inner-Two: 2'] as $expected) {
    if (!in_array($expected, $listed, true)) {
        $fail[] = "headers_list() missing '$expected': " . json_encode($listed);
    }
}

header('X-Inner-Two: replaced');
$two = array_values(array_filter(
    headers_list(),
    static fn(string $h): bool => str_starts_with($h, 'X-Inner-Two:')
));
if ($two !== ['X-Inner-Two: replaced']) {
    $fail[] = 'replaced header did not collapse to one entry: ' . json_encode($two);
}

$meta = json_encode([
    'tag'           => $_GET['tag'] ?? '?',
    'worker_id'     => $worker->id(),
    'request_count' => $worker->requestCount(),
]);

if ($fail !== []) {
    http_response_code(500);
    echo "INNER-FAIL $meta: " . implode('; ', $fail) . "\n";
    return;
}

echo "INNER-OK $meta\n";
