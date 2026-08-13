<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/breaker_probe.php';

// Two fatals, one short of the threshold: the worker gets to keep serving.
//
// The half of the guarantee that says the breaker is bounded — a fix that
// retired the worker on the first error would satisfy "three fatals retire it"
// just as well, and would be wrong. Answering this request also ends the run,
// so the three fatals that follow it in the suite start counting from zero.

$test = new TestCase('breaker_holds_below_threshold', 'breaker');

$worker = OxPHP\Server\Worker::current();

// 1 baseline + 2 fatals + this one. A worker that had been replaced would be
// reporting 1 here.
$test->assertSame('still the worker that served the baseline', $worker->requestCount(), 4);

$recycles = breaker_recycles();
$test->assertNotNull('/metrics exposes the worker-mode block', $recycles);
if ($recycles !== null) {
    $test->assertSame('two fatals recycled nothing', $recycles['total'], 0);
    $test->assertSame('two fatals are below the threshold', $recycles['error'], 0);
}

$test->done();
