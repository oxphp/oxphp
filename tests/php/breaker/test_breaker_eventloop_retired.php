<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/breaker_probe.php';

// The worker that took three fatals from its event-loop tick is gone, and this
// request is answered by its replacement — the same conclusion the fast-path
// probe draws, reached through the other dispatch site.

$test = new TestCase('breaker_eventloop_retired', 'breaker');

$worker = OxPHP\Server\Worker::current();
$test->assertSame('a freshly booted worker is serving', $worker->requestCount(), 1);

$recycles = breaker_recycles();
$test->assertNotNull('/metrics exposes the worker-mode block', $recycles);
if ($recycles !== null) {
    // The two trips from the fast path, and this one from the tick.
    $test->assertSame('a third worker was recycled', $recycles['total'], 3);
    $test->assertSame('and it went for consecutive errors', $recycles['error'], 3);
}

$test->done();
