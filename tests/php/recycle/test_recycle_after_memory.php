<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/recycle_probe.php';

// The worker that passed its ceiling is gone, and the one answering booted
// after it. The counter says why the old one left: reason="max_memory" is
// recorded by the worker thread on its way out, before the pool can notice and
// spawn the replacement that serves this request.

$test = new TestCase('recycle_after_memory', 'recycle');

$worker = OxPHP\Server\Worker::current();
$test->assertSame('a freshly booted worker is serving', $worker->requestCount(), 1);
$test->assertFalse(
    'and it does not carry the ballast',
    isset($sharedState['recycle_ballast'])
);

$counts = recycle_counts();
$test->assertNotNull('/metrics exposes the worker-mode block', $counts);
if ($counts !== null) {
    $test->assertSame('exactly one worker was recycled', $counts['total'], 1);
    $test->assertSame('and it went for its memory ceiling', $counts['max_memory'], 1);
}

$test->done();
