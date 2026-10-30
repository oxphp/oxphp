<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A task that keeps calling Fiber::suspend() after its refusals is ended with a
// graceful exit. Its unwind can still suspend the fiber: a finally block that
// parks on a timer, or one that throws and is caught further out by code that
// suspends again. Either way the fiber is still running that task, so it must
// not be answered for and handed to the next task until the unwind is over.
// One async worker, so the next task is given the same fiber.

$t = new TestCase('task_ended_for_suspending_is_not_reused', 'async');

// The finally block parks: the answer waits for it to finish.
$marker = sys_get_temp_dir() . '/oxphp-task-suspend-park-' . bin2hex(random_bytes(4));
$p = oxphp_async(static function (string $marker): string {
    try {
        while (true) {
            try {
                \Fiber::suspend();
            } catch (\FiberError) {
            }
        }
    } finally {
        oxphp_sleep(0.05);
        file_put_contents($marker, 'ran');
    }
}, $marker);
$message = null;
try {
    oxphp_async_await($p, 5.0);
} catch (\OxPHP\Async\AsyncException $e) {
    $message = $e->getMessage();
}
$t->assertContains('parked unwind: the awaiter is told the task was ended', (string) $message, 'kept retrying');
$t->assertTrue('parked unwind: the answer came after the finally block finished', file_exists($marker));
if (file_exists($marker)) {
    unlink($marker);
}
$t->assertSame('parked unwind: the next task runs its own closure', oxphp_async_await(oxphp_async(static fn (): int => 42), 5.0), 42);

// The finally block throws, a catch further out takes it and suspends again:
// the task is ended again until it stops.
$marker = sys_get_temp_dir() . '/oxphp-task-suspend-escape-' . bin2hex(random_bytes(4));
$p = oxphp_async(static function (string $marker): string {
    for ($i = 0; $i < 100; $i++) {
        try {
            try {
                \Fiber::suspend();
            } catch (\FiberError) {
            } finally {
                throw new \RuntimeException('thrown from finally');
            }
        } catch (\RuntimeException) {
        }
    }
    file_put_contents($marker, 'ran');

    return 'ran to its end';
}, $marker);
$message = null;
$value = 'not awaited';
try {
    $value = oxphp_async_await($p, 5.0);
} catch (\OxPHP\Async\AsyncException $e) {
    $message = $e->getMessage();
}
$t->assertContains('escaped unwind: the awaiter is told the task was ended', (string) $message, 'kept retrying');
$t->assertSame('escaped unwind: no value was delivered', $value, 'not awaited');
$t->assertTrue('escaped unwind: the answer came after the task stopped suspending', file_exists($marker));
if (file_exists($marker)) {
    unlink($marker);
}
$t->assertSame('escaped unwind: the next task runs its own closure', oxphp_async_await(oxphp_async(static fn (): int => 43), 5.0), 43);

// The finally block throws and nothing catches it: the task ends with that
// exception, which its awaiter is given in place of the server's reason.
$p = oxphp_async(static function (): string {
    try {
        while (true) {
            try {
                \Fiber::suspend();
            } catch (\FiberError) {
            }
        }
    } finally {
        throw new \RuntimeException('thrown while being ended');
    }
});
$message = null;
try {
    oxphp_async_await($p, 5.0);
} catch (\OxPHP\Async\AsyncException $e) {
    $message = $e->getMessage();
}
$t->assertContains('thrown unwind: the awaiter gets what the task ended with', (string) $message, '[RuntimeException] thrown while being ended');
$t->assertSame('thrown unwind: the next task runs its own closure', oxphp_async_await(oxphp_async(static fn (): int => 44), 5.0), 44);

$t->done();
