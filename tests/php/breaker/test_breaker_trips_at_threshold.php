<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/breaker_probe.php';

// Three fatals in a row, none of them suspending: the worker is retired and
// this request is answered by its replacement.
//
// Both readings are needed. requestCount() == 1 says the worker answering now
// booted after those fatals; the recycle counter says why the old one left —
// reason="error" is this breaker, and it is recorded by the worker thread on
// its way out, before the pool can notice it died and spawn the replacement
// that serves this request.

$test = new TestCase('breaker_trips_at_threshold', 'breaker');

$worker = OxPHP\Server\Worker::current();
$test->assertSame('a freshly booted worker is serving', $worker->requestCount(), 1);

$recycles = breaker_recycles();
$test->assertNotNull('/metrics exposes the worker-mode block', $recycles);
if ($recycles !== null) {
    $test->assertSame('exactly one worker was recycled', $recycles['total'], 1);
    $test->assertSame('and it went for consecutive errors', $recycles['error'], 1);
}

$test->done();
