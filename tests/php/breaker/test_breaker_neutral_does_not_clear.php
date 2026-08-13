<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/breaker_probe.php';

// Neutral means neutral: a cancellation between two fatals does not clear the
// run, and the third fatal still retires the worker.
//
// This is the half of the classification the other probes cannot see. They show
// that a cancellation on its own does not retire a worker — which a rule that
// silently reset the count on every cancellation would satisfy just as well, and
// that rule would hand any client a way to keep a genuinely broken worker alive
// forever: one disconnect between fatals and the count never reaches three.
//
// The lines above this one are fatal, fatal, timeout, fatal.

$test = new TestCase('breaker_neutral_does_not_clear', 'breaker');

$worker = OxPHP\Server\Worker::current();
$test->assertSame('a freshly booted worker is serving', $worker->requestCount(), 1);

$recycles = breaker_recycles();
$test->assertNotNull('/metrics exposes the worker-mode block', $recycles);
if ($recycles !== null) {
    // The trip earlier in the suite, and this one.
    $test->assertSame('a second worker was recycled', $recycles['total'], 2);
    $test->assertSame('and it went for consecutive errors', $recycles['error'], 2);
}

$test->done();
