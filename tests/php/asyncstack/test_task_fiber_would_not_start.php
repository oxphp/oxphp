<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// fiber.stack_size is below the engine's minimum in this profile, so the
// engine refuses to start the fiber a task runs in, and says why in an
// exception. That exception is the task's outcome: the request awaiting the
// task is handed it, and nothing else is affected. The engine raises it on the
// async worker's scheduler, outside any PHP frame, where an exception left to
// the engine is reported as an uncaught fatal and ends the whole process —
// every request in flight with it, this one included.

$t = new TestCase('task_fiber_would_not_start', 'asyncstack');

$caught = null;
try {
    oxphp_async_await(oxphp_async(static fn (): int => 1), 5.0);
} catch (\Throwable $e) {
    $caught = $e;
}

$t->assertInstanceOf('the await reports the task as failed', $caught, \OxPHP\Async\AsyncException::class);
$t->assertContains(
    "the failure is the engine's refusal of the fiber's stack",
    (string) $caught?->getMessage(),
    'Fiber stack size is too small'
);
$t->done();
