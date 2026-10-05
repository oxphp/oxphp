<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// From PHP 8.5 a fatal takes a backtrace of where it was raised, with the
// arguments of every frame, and the engine keeps it on the thread until the next
// error there. An oxphp_async() task that fatals ends there, but the thread it
// ran on goes on running other tasks — so whatever that backtrace holds has to
// be let go of when the task ends, not whenever the thread next raises an error.
//
// Measured from a task on the same thread: this profile runs one async worker.
// A notice is an error, and any error lets go of the backtrace standing on the
// thread, so the heap shrinking across one is what that backtrace was still
// holding. A third task stays parked across the measurement: with nothing in
// flight the pool resets the thread between tasks, and that reset could hide
// what the end of the task left.

$t = new TestCase('async_task_fatal_backtrace_is_released', 'fibers');

/** The size of what the fatal task's frame is given, as the task spells it. */
$blobBytes = 1 << 20;

$sleeper = oxphp_async(static function (): array {
    $start = hrtime(true);
    oxphp_sleep(1.5);

    return [$start, hrtime(true)];
});

$fatal = oxphp_async(static function (): void {
    set_error_handler(null);
    $fail = static function (string $blob): void {
        trigger_error('async task backtrace probe fatal', E_USER_ERROR);
    };
    $fail(str_repeat('x', 1 << 20));
});

$fatalThrew = false;
try {
    oxphp_async_await($fatal, 5.0);
} catch (\Throwable $e) {
    $fatalThrew = true;
}

$measure = oxphp_async(static function (): array {
    set_error_handler(null);
    $before = memory_get_usage();
    @trigger_error('async task backtrace probe notice', E_USER_NOTICE);
    $after = memory_get_usage();

    return [hrtime(true), $before - $after];
});

[$measuredAt, $freed] = oxphp_async_await($measure, 5.0);
[$sleepStart, $sleepEnd] = oxphp_async_await($sleeper, 5.0);

$t->assertTrue('the fatal task ended in a fatal', $fatalThrew);

// What the measurement stands on: another task was in flight while it ran, read
// from both edges of that task rather than from a flag it only ever raises.
$t->assertTrue('the sleeping task had started before the measurement', $sleepStart < $measuredAt);
$t->assertTrue('and had not finished yet', $measuredAt < $sleepEnd);

$t->assertLessThan(
    'nothing the fatal task\'s frames were given is still held on the thread (' . $freed . ' bytes let go of)',
    $freed,
    $blobBytes / 16
);

$t->done();
