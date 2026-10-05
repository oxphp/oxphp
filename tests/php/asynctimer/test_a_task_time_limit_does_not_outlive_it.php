<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('a_task_time_limit_does_not_outlive_it', 'asynctimer');

// A time limit one task sets must not be left running over the next one.
//
// set_time_limit() inside a task goes through the engine's ini handler, which
// arms the thread's execution timer — and the timer is one per thread, shared by
// every task that runs on it. A limit armed there therefore belongs to no task
// in particular: it counts down against whichever one is executing opcodes when
// it runs out, and the task that asked for it may well be parked by then.
//
// Here the first task arms two seconds and parks for three, so the mark falls
// while the second task — which asked for no limit at all — is running.
//
// This runs after test_a_task_across_the_mark_survives on purpose. On a build
// that leaves the startup arm standing — which is the build that test catches —
// it needs the thread's own startup mark still pending, and the
// set_time_limit() below spends it: the thread has one execution timer and it
// is one-shot, so arming re-arms that same timer rather than adding a second
// one. The suite file keeps the order.

// Arms two seconds and parks past them, so the mark falls inside the sibling.
$first = oxphp_async(function (): array {
    set_time_limit(2);
    $armed_at = microtime(true);
    $limit = (string) ini_get('max_execution_time');
    oxphp_sleep(3.0);
    return ['limit' => $limit, 'armed_at' => $armed_at];
});

// Let the first task reach its park before the second is dispatched, so the
// second is admitted with the first still in flight.
usleep(200_000);

$second = oxphp_async(function (): array {
    $start = microtime(true);
    $end = $start + 2.5;
    while (microtime(true) < $end) {
    }
    return ['start' => $start, 'end' => microtime(true)];
});

$second_seen = null;
$second_err = '';
try {
    $second_seen = oxphp_async_await($second, 20.0);
} catch (\Throwable $e) {
    $second_err = get_class($e) . ': ' . $e->getMessage();
}

$first_seen = null;
$first_err = '';
try {
    $first_seen = oxphp_async_await($first, 20.0);
} catch (\Throwable $e) {
    $first_err = get_class($e) . ': ' . $e->getMessage();
}

// Premise: the first task really did ask for a two-second limit. The ini handler
// is the path that arms, and this is the value it was handed — which the
// directive also keeps reading back, since what a pool thread gives up is the
// deadline, not the value.
$t->assertSame('the first task armed a two-second limit', $first_seen['limit'] ?? null, '2');

$t->assertSame('the second task reported no error', $second_err, '');
$t->assertSame('the first task reported no error', $first_err, '');

// Premise: the second task was executing opcodes when that limit ran out.
$mark = ($first_seen['armed_at'] ?? 0.0) + 2.0;
$spanned = isset($second_seen['start'], $second_seen['end'])
    && $second_seen['start'] < $mark
    && $mark < $second_seen['end'];
$t->assertTrue('the second task was running across that limit', $spanned);

$t->done();
