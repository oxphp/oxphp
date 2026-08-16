<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('task_sleep_precision', 'async');

// The async pool's driver widens its idle wait while nothing is happening, so
// that a task parked on a long timer does not cost a wakeup every millisecond.
// A sleeping fiber's wake-up time is not among the things that widening is for:
// the worker set that timer itself and knows exactly when it expires, so the
// wait is cut short to land on it instead of running past it.
//
// The overshoot per sleep is small — a fraction of the widened wait — so it is
// summed over many sleeps rather than asserted on one: measured over 20 sleeps
// of 100 ms, 2019 ms when the wait is cut short and 2118 ms when it is not.
// A single sample overlaps between the two (a sleep whose deadline happens to
// fall near the end of a wait overshoots by almost nothing either way), which
// is exactly why the sum is the statistic and not the maximum.
$p = oxphp_async(function (): array {
    $samples = [];
    for ($i = 0; $i < 20; $i++) {
        $started = microtime(true);
        oxphp_sleep(0.1);
        $samples[] = (microtime(true) - $started) * 1000;
    }
    return $samples;
});

$samples = oxphp_async_await($p, 30.0);
$total   = array_sum($samples);

$t->assertSame('all 20 sleeps ran', count($samples), 20);

// 20 sleeps of 100 ms cannot take less than 2000 ms; below that the sleep did
// not happen and the upper bound would be meaningless.
$t->assertGreaterThan('the sleeps actually slept', $total, 1995.0);

// Sits between the two measurements above, with about 50 ms of room on each
// side — two and a half milliseconds per sleep for a machine under load.
$t->assertLessThan('sleeps are not stretched by the pool idle interval', $total, 2070.0);

$t->done();
