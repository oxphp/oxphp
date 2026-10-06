<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/recycle_probe.php';

// The worker that scheduled its exit is gone, and the one answering booted
// after it; the second recycle in this profile is counted under "scheduled".

$test = new TestCase('recycle_after_scheduled', 'recycle');

$worker = OxPHP\Server\Worker::current();
$test->assertSame('a freshly booted worker is serving', $worker->requestCount(), 1);

$counts = recycle_counts();
$test->assertNotNull('/metrics exposes the worker-mode block', $counts);
if ($counts !== null) {
    $test->assertSame('two workers have been recycled', $counts['total'], 2);
    $test->assertSame('the second one on request', $counts['scheduled'], 1);
}

$test->done();
