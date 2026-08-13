<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/breaker_probe.php';

// The other half of neutral, and the half that a request nobody marks fails.
//
// Not retiring a worker is satisfied just as well by a rule that quietly reads
// such a request as a success — and that rule hands any application a way to
// keep a worker that fatals on every other request: one shutdown function that
// throws, in between, and the run never reaches three. This is the sequence that
// tells the two apart.
//
// The lines above this one are fatal, shutdown throw, fatal, fatal.

$test = new TestCase('breaker_shutdown_throw_does_not_clear', 'breaker');

$worker = OxPHP\Server\Worker::current();
$test->assertSame('a freshly booted worker is serving', $worker->requestCount(), 1);

$recycles = breaker_recycles();
$test->assertNotNull('/metrics exposes the worker-mode block', $recycles);
if ($recycles !== null) {
    $test->assertSame('an eighth worker was recycled', $recycles['total'], 8);
    $test->assertSame('and it went for consecutive errors', $recycles['error'], 8);
}

$test->done();
