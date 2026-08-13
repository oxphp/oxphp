<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/breaker_probe.php';

// The state every other test in this profile is measured against: worker mode
// is on, this is the first request the worker has served, and no worker has
// been recycled for any reason yet.

$test = new TestCase('breaker_baseline', 'breaker');

$test->assertTrue('worker mode is active', oxphp_is_worker());

$worker = OxPHP\Server\Worker::current();
$test->assertSame('first request on this worker', $worker->requestCount(), 1);

$recycles = breaker_recycles();
$test->assertNotNull('/metrics exposes the worker-mode block', $recycles);
if ($recycles !== null) {
    $test->assertSame('no recycles yet', $recycles['total'], 0);
    $test->assertSame('no error recycles yet', $recycles['error'], 0);
}

$test->done();
