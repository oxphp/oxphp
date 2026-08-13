<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/breaker_probe.php';

// Three requests whose shutdown functions fatalled: the worker is retired and
// this request is answered by its replacement.
//
// A fatal raised past the arm that catches the handler's own is still a fatal —
// the engine has abandoned frames on the VM stack and cleared its execution
// cursor, and the request that follows on this worker inherits both. Before the
// fix such a request reached the breaker with no flag raised at all, which is
// the branch that reports a healthy worker: the count was reset, and an
// application fataling in its shutdown function on every single request read as
// the picture of health.

$test = new TestCase('breaker_shutdown_retires', 'breaker');

$worker = OxPHP\Server\Worker::current();
$test->assertSame('a freshly booted worker is serving', $worker->requestCount(), 1);

$recycles = breaker_recycles();
$test->assertNotNull('/metrics exposes the worker-mode block', $recycles);
if ($recycles !== null) {
    // The three trips earlier in the suite, and this one.
    $test->assertSame('a fourth worker was recycled', $recycles['total'], 4);
    $test->assertSame('and it went for consecutive errors', $recycles['error'], 4);
}

$test->done();
