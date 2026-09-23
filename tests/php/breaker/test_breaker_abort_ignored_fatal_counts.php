<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/breaker_probe.php';

// Three fatals raised on requests whose clients had already gone: the worker is
// retired and this request is answered by its replacement.
//
// The counterpart of the probe above. The fatal's message is written while the
// cancel cell holds the client's departure, and in worker mode that write lets a
// request that is not streaming go on — so the fatal runs its course and is
// filed as what it is. Filed as a cancellation instead, it would be neutral, and
// a worker running an application that fatals on every request would never be
// retired for as long as clients kept giving up on it, which is a load shape
// that produces cancellations by the hundred.

$test = new TestCase('breaker_abort_ignored_fatal_counts', 'breaker');

$worker = OxPHP\Server\Worker::current();
$test->assertSame('a freshly booted worker is serving', $worker->requestCount(), 1);

$recycles = breaker_recycles();
$test->assertNotNull('/metrics exposes the worker-mode block', $recycles);
if ($recycles !== null) {
    // The eight trips earlier in the suite, and this one.
    $test->assertSame('a ninth worker was recycled', $recycles['total'], 9);
    $test->assertSame('and it went for consecutive errors', $recycles['error'], 9);
}

$test->done();
