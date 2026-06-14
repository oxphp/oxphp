<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('async_cap_reject', 'async');

// Profile asynccap: ASYNC_WORKERS=1, ASYNC_MAX_FIBERS=2. The process-global
// in-flight bound (queued + running) is async_max_fibers × workers = 2. A third
// concurrent dispatch must be rejected — non-blocking — with AsyncException
// while the first two are still in flight. Once they drain, capacity frees and
// a fresh dispatch succeeds. The counter is bumped at dispatch on this request
// thread, so the result is deterministic regardless of worker scheduling: the
// two blocking tasks (usleep) cannot complete and release before the third
// dispatch runs microseconds later on the same thread.

$id1 = oxphp_async(function (): int {
    usleep(300000);
    return 1;
});
$id2 = oxphp_async(function (): int {
    usleep(300000);
    return 2;
});
$t->assertTrue('first two dispatches accepted', $id1 >= 0 && $id2 >= 0);

$t->assertThrows(
    'third dispatch rejected at capacity',
    function (): void {
        oxphp_async(fn (): int => 3);
    },
    \OxPHP\Async\AsyncException::class
);

// Drain the two in-flight tasks — each release frees one permit.
$t->assertSame('first task result', oxphp_async_await($id1, 3.0), 1);
$t->assertSame('second task result', oxphp_async_await($id2, 3.0), 2);

// Capacity is free again, so a new dispatch is accepted and resolves.
$id4 = oxphp_async(fn (): int => 4);
$t->assertTrue('dispatch accepted after capacity frees', $id4 >= 0);
$t->assertSame('fourth task result', oxphp_async_await($id4, 3.0), 4);

$t->done();
