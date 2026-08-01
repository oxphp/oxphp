<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('await_timeout_in_request_fiber', 'worker');

// Worker-mode request handlers run inside a request fiber driven by the HTTP
// scheduler. A per-call await timeout must unwind the await when the awaited
// promise does not settle in time — otherwise the request blocks until the
// task completes, silently ignoring the timeout argument.
$p = oxphp_async(function (): int {
    oxphp_sleep(1.0); // settles long after the 0.2s await deadline below
    return 42;
});

$start = microtime(true);
$t->assertThrows(
    'await($p, 0.2) in a request fiber throws TimeoutException',
    function () use ($p) {
        oxphp_async_await($p, 0.2);
    },
    \OxPHP\Async\TimeoutException::class
);
$elapsed = microtime(true) - $start;

// The await must unwind at its own 0.2s deadline, not after the 1s task. Proves
// the HTTP scheduler honours await_deadline_ns — regression guard for the bug
// where it ignored the deadline and blocked until the promise settled.
$t->assertLessThan('await unwound at its deadline, not after the task', $elapsed, 0.8);

$t->done();
