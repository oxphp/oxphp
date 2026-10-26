<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// The task half of fibers/test_request_fiber_stays_current_after_a_fatal_in_a_fiber:
// an oxphp_async() task that fatals inside a Fiber it started leaves the fiber
// it ran in able to say so, for every later task the async worker runs on it.
//
// Measured on the async worker thread, which this profile runs one of, so every
// task lands on the same thread and object ids are comparable between them.
// A fourth task stays parked across the three: with nothing in flight the pool
// resets the thread between tasks, and that reset could hide what the fatal
// left. The tasks have to land on the same
// fiber for this to ask anything, so the one that fatals records its fiber where
// the next task can read it, and that is checked rather than assumed.

$t = new TestCase('async_task_fiber_stays_current_after_a_fatal_in_a_fiber', 'fibers');

// Queued first, so it is also what clears what an earlier run left.
$sleeper = oxphp_async(static function (): void {
    require_once __DIR__ . '/fixture_async_task_fiber_current.php';
    OxphpAsyncTaskFiberCurrentProbe::$fatalFiber = null;
    oxphp_sleep(1.5);
});

$ident = static function (): array {
    // The task runs on another thread, which has functions and classes of its own.
    require_once __DIR__ . '/fixture_async_task_fiber_current.php';
    $current = \Fiber::getCurrent();

    return [
        'fiber' => $current === null ? null : spl_object_id($current),
        'fatal_fiber' => OxphpAsyncTaskFiberCurrentProbe::$fatalFiber,
    ];
};

$before = oxphp_async_await(oxphp_async($ident), 5.0);

$fatal = oxphp_async(static function (): void {
    require_once __DIR__ . '/fixture_async_task_fiber_current.php';
    require_once __DIR__ . '/../engine_fatal.php';
    $current = \Fiber::getCurrent();
    OxphpAsyncTaskFiberCurrentProbe::$fatalFiber = $current === null ? null : spl_object_id($current);

    (new \Fiber(static function (): void {
        engine_fatal('OxphpAsyncTaskFiberCurrent');
    }))->start();
});

$fatalMessage = null;
try {
    oxphp_async_await($fatal, 5.0);
} catch (\Throwable $e) {
    $fatalMessage = $e->getMessage();
}

$after = oxphp_async_await(oxphp_async($ident), 5.0);
oxphp_async_await($sleeper, 5.0);

$t->meta('before', $before);
$t->meta('after', $after);

$t->assertContains(
    'the task that started a fiber ended in the fatal raised inside it',
    (string) $fatalMessage,
    'OxphpAsyncTaskFiberCurrent'
);

$fiber = $before['fiber'] ?? null;
$t->assertNotNull('the first task ran in a fiber', $fiber);
$t->assertSame('the task that fataled ran in the same one', $after['fatal_fiber'] ?? null, $fiber);
$t->assertSame('the next task on that fiber is told it runs in it', $after['fiber'] ?? null, $fiber);

$t->done();
