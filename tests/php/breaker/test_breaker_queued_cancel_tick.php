<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/queued_cancel.php';

// The same three requests, reached the other way: by the event loop's tick
// while this request is parked.
//
// Once they are staged, this request sleeps in oxphp_sleep(), which is what
// puts the worker on its event-loop branch — there is a fiber to come back to,
// so the loop ticks instead of waiting, and the tick is what takes the queue's
// entries. They must be dropped there as well, and dropped before any of them
// is given a fiber.

$test = new TestCase('breaker_queued_cancel_tick', 'breaker');

$before = queued_cancel_stage($test);

if ($before !== null) {
    // oxphp_sleep() rather than usleep(): this profile runs no hooks, so a
    // native sleep would block the worker and the tick would never run. Half a
    // second is long enough for it to take three entries many times over.
    oxphp_sleep(0.5);

    $m = queued_cancel_metrics();
    $test->assertNotNull('/metrics is readable', $m);
    if ($m !== null) {
        // Empty, or the tick never took them and nothing below means anything.
        $test->assertSame('the tick took all three from the queue', $m['queue_depth'], 0);

        $test->assertSame('none of the three was run', queued_cancel_victims_run(), 0);

        // This request is still running, so it is not in this count either.
        $test->assertSame('none of the three was counted as handled', $m['handled'], $before['handled']);
    }
}

$test->done();
