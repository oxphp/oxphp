<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('a_task_across_the_mark_survives', 'asynctimer');

// A task that happens to be running opcodes when the pool thread's execution
// timer runs out must not be ended by it.
//
// The thread arms that timer in the one php_request_startup() it runs at server
// start, with max_execution_time (10s here, see zz-asynctimer.ini), and nothing
// used to take it off again. So roughly ten seconds into the process the engine
// ended whichever task was executing then — reported as "Maximum execution time
// of 0 seconds exceeded", the zero being EG(timeout_seconds) after the reset
// between tasks zeroed it. The task is in no way at fault: the deadline is the
// thread's, not its own.
//
// Neither edge of the window is assumed, and both can fail:
//
//  - The far edge is measured by the task itself. The thread cannot run a task
//    before its own startup has returned, so it armed no later than this task's
//    first opcode; spinning for longer than the limit therefore reaches past
//    the mark wherever the arm was.
//  - The near edge is read from the uptime gauge, whose clock starts before the
//    pool is started at all. So the arm is no earlier than uptime zero, and a
//    dispatch made while the uptime is still below the limit is before the
//    mark.

/** Server uptime in whole seconds; -1 when the endpoint is unreachable. */
function server_uptime(): int {
    $m = @file_get_contents('http://127.0.0.1:9090/metrics');
    if (!is_string($m)) {
        return -1;
    }
    if (preg_match('/^oxphp_uptime_seconds\s+(\d+)$/m', $m, $mm)) {
        return (int) $mm[1];
    }
    return -1;
}

$limit = 10;                   // max_execution_time of this profile
$spin  = (float) $limit + 1.0; // one second past the latest the mark can be

// The profile's limit bounds this request as well, counted from its own start,
// and a task held for longer than the limit cannot be held from inside a
// request that short. This thread's deadline is not what is under test, so it
// is lifted out of the way; the runner's own 15s cap still bounds the run.
set_time_limit(30);

// Premise: the mark is still ahead of the task about to be dispatched. The
// gauge is floored, so "8" means somewhere in [8.0, 9.0) — reading below
// $limit - 1 therefore leaves a whole second for the dispatch itself. On a
// runner so slow that the profile turned healthy past that, this fails with the
// uptime in the message rather than passing without having tested anything.
$uptime = server_uptime();
$t->assertTrue('metrics endpoint reachable', $uptime >= 0);
$t->assertLessThan("the {$limit}s mark is still ahead (uptime {$uptime}s)", $uptime, $limit - 1);

$p = oxphp_async(function () use ($spin): array {
    // A tight loop, so the task is executing opcodes for the whole window and
    // the engine has a back edge to raise the fatal on.
    $start = microtime(true);
    $end = $start + $spin;
    while (microtime(true) < $end) {
    }
    return ['start' => $start, 'end' => microtime(true)];
});

$seen = null;
$err = '';
try {
    $seen = oxphp_async_await($p, 20.0);
} catch (\Throwable $e) {
    $err = get_class($e) . ': ' . $e->getMessage();
}

$t->assertSame('the task spanning the mark reported no error', $err, '');
$t->assertTrue('the task spanning the mark completed', isset($seen['start'], $seen['end']));

// Premise: the window really did reach past the latest the mark could be. A
// spin cut short fails here, instead of passing as a task that was never under
// the thread's deadline in the first place.
$ran = ($seen['end'] ?? 0.0) - ($seen['start'] ?? 0.0);
$t->assertGreaterThan("the window outlasted the {$limit}s limit (ran {$ran}s)", $ran, (float) $limit);

$t->done();
