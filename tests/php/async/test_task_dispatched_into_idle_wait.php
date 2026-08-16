<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('task_dispatched_into_idle_wait', 'async');

// A worker whose fibers are all parked spends its idle interval waiting on the
// task queue, so a task dispatched during that interval is taken off the queue
// by the wait itself rather than by the poll at the top of the loop. That task
// is then held until the next turn, which is the only moment it can be run —
// and if it were dropped there instead, nothing would report it: the awaiter
// would simply find its result channel closed, which is indistinguishable from
// a task that failed for a reason of its own.
//
// One async worker, so the parked task and the dispatched one are the same
// worker's problem. The first task parks for long enough that the idle wait has
// widened and is certainly the state the second task arrives into.
$parked = oxphp_async(function (): int {
    oxphp_sleep(1.0);
    return 1;
});

// Long enough to be inside the widened wait, short enough to be well before the
// parked task wakes.
usleep(200_000);

$arriving = oxphp_async(fn (): string => 'ran');

$got    = null;
$threw  = '';
try {
    $got = oxphp_async_await($arriving, 5.0);
} catch (\Throwable $e) {
    $threw = get_class($e) . ': ' . $e->getMessage();
}

$t->assertSame('task dispatched into the idle wait ran and returned its value', $got, 'ran');
$t->assertSame('and did not come back as a failure', $threw, '');

// The parked task is still owed an answer of its own — a dropped or
// double-counted task would take its in-flight permit with it.
$t->assertSame('the parked task still completes', oxphp_async_await($parked, 5.0), 1);

$t->done();
