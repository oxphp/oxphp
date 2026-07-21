<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('sleep_task_multiplex', 'hooks');

// ASYNC_WORKERS=1: three tasks each native-sleep(1). Hooked sleep suspends
// the fiber, so one worker thread interleaves all three → total ~1s.
// Blocking sleep would serialize them → total ~3s.
$t0 = microtime(true);
$tasks = [];
for ($i = 0; $i < 3; $i++) {
    $tasks[] = oxphp_async(function (): int {
        return sleep(1);
    });
}
$results = oxphp_async_await_all($tasks);
$elapsed = microtime(true) - $t0;

$t->assertSame('every hooked sleep returned 0', $results, [0, 0, 0]);
$t->assertTrue('sleeps overlapped on one worker (elapsed < 2.0s, serial would be >= 3s)',
    $elapsed < 2.0);
$t->assertTrue('sleeps actually slept (elapsed >= 1.0s)', $elapsed >= 1.0);

$t->done();
