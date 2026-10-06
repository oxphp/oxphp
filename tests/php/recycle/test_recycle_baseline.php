<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/recycle_probe.php';

// The state the rest of this profile is measured against: worker mode is on,
// this is the first request the worker has served, nothing has been recycled
// yet, and the worker sits well below its memory ceiling — so the recycle the
// next block provokes is that block's doing and not the worker entry's own
// footprint.

$test = new TestCase('recycle_baseline', 'recycle');

$test->assertTrue('worker mode is active', oxphp_is_worker());

$worker = OxPHP\Server\Worker::current();
$test->assertSame('first request on this worker', $worker->requestCount(), 1);
$test->assertSame('the memory ceiling is 16 MiB', $worker->maxMemoryBytes(), 16 * 1024 * 1024);
$test->assertLessThan('the worker is well below its ceiling', memory_get_usage(), 8 * 1024 * 1024);

$counts = recycle_counts();
$test->assertNotNull('/metrics exposes the worker-mode block', $counts);
if ($counts !== null) {
    $test->assertSame('no recycles yet', $counts['total'], 0);
}

$test->done();
