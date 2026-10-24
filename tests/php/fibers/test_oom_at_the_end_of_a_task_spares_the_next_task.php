<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// An oxphp_async() task that runs out of memory at its very end — after its
// result has been taken, in the report of an exception a destructor threw as the
// task let go of its shutdown functions — still has the result it returned, and
// the next task on the thread runs as if nothing had happened.
//
// From PHP 8.4.27 / 8.5.12 the engine reports running out of memory from a point
// where it can unwind the code that did, and keeps a note that it has until the
// request is shut down; while the note stands it calls no callback and runs no
// destructor. It also lets an allocation past the limit through and reports it
// only at the next point that looks: a call, or the VM's next interrupt check.
// ?case=handled runs out inside the exception's __toString(), which leaves the
// note; ?case=pending runs out in the C code that builds the report from what
// __toString() returned, which leaves the error unreported. A task ends with
// neither looked for, and the next task on the thread — the call to its
// closure — was handed it and failed in its place: with the out-of-memory
// error it never raised for ?case=pending, and for ?case=handled with an
// "Uncaught ... thrown" error that names nothing.
//
// Neither displayed nor logged. A task's ini changes stay on its thread, so the
// last task puts them back. This profile runs one async worker, so every task
// here runs on the same thread.

$t = new TestCase('oom_at_the_end_of_a_task_spares_the_next_task', 'fibers');

$case = ($_GET['case'] ?? 'handled') === 'pending' ? 'pending' : 'handled';
$marker = sys_get_temp_dir() . '/oxphp-task-late-oom-' . bin2hex(random_bytes(6));

$late = oxphp_async(static function (string $marker, string $case): string {
    set_error_handler(null);
    ini_set('display_errors', '0');
    ini_set('log_errors', '0');
    if ($case === 'pending') {
        // Room for the string __toString() returns and not for the report's
        // copy of it, which is what runs out. Low, because the server logs
        // the report, and the string is in it.
        ini_set('memory_limit', (string) (memory_get_usage(true) + (8 << 20)));
    }
    require_once __DIR__ . '/task_late_oom_thrower.php';

    // A shutdown function the task registers holds the object; the free of the
    // task's registrations at its end drops it.
    $holder = new OxphpTaskLateOomHolder($marker, $case);
    register_shutdown_function(static function () use ($holder): void {
    });
    unset($holder);

    return 'returned';
}, $marker, $case);

$t->assertSame("the task's outcome is what its closure returned", oxphp_async_await($late, 20.0), 'returned');

$next = oxphp_async(static fn (): int => 42);
try {
    $value = oxphp_async_await($next, 20.0);
} catch (\Throwable $e) {
    $value = get_class($e) . ': ' . $e->getMessage();
}

// What the rest stands on: the destructor ran at the end of the first task, and
// the report of what it threw called __toString(). One thread runs both tasks,
// so the first has ended by the time the second has.
$t->assertSame(
    'the exception the destructor threw at the end of the task was reported',
    is_file($marker) ? file_get_contents($marker) : 'no mark',
    'destructor tostring'
);

$t->assertSame('the next task on the thread returns its own value', $value, 42);

oxphp_async_await(oxphp_async(static function (): void {
    ini_restore('display_errors');
    ini_restore('log_errors');
    ini_restore('memory_limit');
    // The "pending" report keeps its 4 MiB message as the thread's last error.
    error_clear_last();
}), 5.0);

if (is_file($marker)) {
    unlink($marker);
}

$t->done();
