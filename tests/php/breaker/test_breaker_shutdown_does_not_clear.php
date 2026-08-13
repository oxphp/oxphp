<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/breaker_probe.php';

// The half the block above cannot see: a shutdown-function fatal between two
// handler fatals does not clear the run either.
//
// This is the worse of the two harms the case carried. A request counted as a
// success resets the count, so an application alternating between the two kinds
// of fatal — one in the handler, one in the shutdown function — held the counter
// at one forever and kept a worker that failed every single request in the pool
// for the life of the process. A rule that merely stopped short of counting this
// request, treating it as neutral, would leave that alternation at two and still
// never reach the threshold; only counting it closes the sequence.
//
// The lines above this one are fatal, shutdown fatal, fatal.

$test = new TestCase('breaker_shutdown_does_not_clear', 'breaker');

$worker = OxPHP\Server\Worker::current();
$test->assertSame('a freshly booted worker is serving', $worker->requestCount(), 1);

$recycles = breaker_recycles();
$test->assertNotNull('/metrics exposes the worker-mode block', $recycles);
if ($recycles !== null) {
    $test->assertSame('a fifth worker was recycled', $recycles['total'], 5);
    $test->assertSame('and it went for consecutive errors', $recycles['error'], 5);
}

$test->done();
