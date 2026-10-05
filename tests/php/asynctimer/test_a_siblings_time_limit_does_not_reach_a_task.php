<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('a_siblings_time_limit_does_not_reach_a_task', 'asynctimer');

// A limit armed by a task that is already in flight must not reach a sibling.
//
// This is the same guarantee as test_a_task_time_limit_does_not_outlive_it, from
// a side that no task boundary can cover. That test arms before the sibling is
// dispatched. Here the arming happens *after* every in-flight task was
// dequeued: the worker runs the dequeue block first and drives suspended fibers
// afterwards, so a task resumed by that drive arms the thread's one timer with
// no dequeue left ahead of it. Dropping the timer between tasks would leave
// this case open; dropping it where it is armed closes both.
//
// A parks, B is dequeued and parks, then A is resumed and only then calls
// set_time_limit(1). B resumes afterwards and runs opcodes across the mark.

// Arms only after its first park, i.e. after B has been dequeued.
$a = oxphp_async(function (): array {
    oxphp_sleep(0.2);
    set_time_limit(1);
    $armed_at = microtime(true);
    $limit = (string) ini_get('max_execution_time');
    // Stay parked past the mark, so A is never the one executing when it falls.
    oxphp_sleep(5.0);
    return ['limit' => $limit, 'armed_at' => $armed_at];
});

// Let A reach its park, so B is admitted with A in flight.
usleep(50_000);

$b = oxphp_async(function (): array {
    $body_start = microtime(true);
    // Resume after A has armed, then hold the CPU across the mark.
    oxphp_sleep(0.5);
    $spin_start = microtime(true);
    $end = $spin_start + 2.0;
    while (microtime(true) < $end) {
    }
    return [
        'body_start' => $body_start,
        'spin_start' => $spin_start,
        'spin_end' => microtime(true),
    ];
});

$b_seen = null;
$b_err = '';
try {
    $b_seen = oxphp_async_await($b, 20.0);
} catch (\Throwable $e) {
    $b_err = get_class($e) . ': ' . $e->getMessage();
}

$a_seen = null;
$a_err = '';
try {
    $a_seen = oxphp_async_await($a, 20.0);
} catch (\Throwable $e) {
    $a_err = get_class($e) . ': ' . $e->getMessage();
}

// Premise: A really did ask for a one-second limit; this is the value the ini
// handler was handed, which is the path that arms.
$t->assertSame('the sibling armed a one-second limit', $a_seen['limit'] ?? null, '1');

$t->assertSame('the task that set no limit reported no error', $b_err, '');
$t->assertSame('the sibling that set the limit reported no error', $a_err, '');
$t->assertTrue('the task that set no limit completed', is_array($b_seen));

// Premise: B was already dequeued and running when A armed — which is what puts
// this case beyond anything a task boundary could have done for B.
$armed_at = $a_seen['armed_at'] ?? 0.0;
$t->assertTrue(
    'the sibling armed after this task was already running',
    isset($b_seen['body_start']) && $b_seen['body_start'] < $armed_at
);

// Premise: the mark that limit set fell inside B's window of executing opcodes.
$mark = $armed_at + 1.0;
$spanned = isset($b_seen['spin_start'], $b_seen['spin_end'])
    && $b_seen['spin_start'] < $mark
    && $mark < $b_seen['spin_end'];
$t->assertTrue('this task was running opcodes across that mark', $spanned);

$t->done();
