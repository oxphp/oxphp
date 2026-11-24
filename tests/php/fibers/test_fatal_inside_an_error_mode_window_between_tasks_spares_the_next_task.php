<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// An async thread runs PHP between tasks, under no PHP frame: the destructors of
// what a finished task returned or held run as the thread lets go of it. A fatal
// there inside an open that runs in the mode turning warnings into exceptions —
// `new DirectoryIterator()` here, `new PDO()` alike — never reaches the line
// that puts the previous mode back. The next task the thread takes still starts
// in the normal mode. Here the end of one task lets go of an error handler the
// task called and removed, the last thing holding an object whose destructor
// fatals inside such an open. ASYNC_WORKERS is 1 in this profile.

$t = new TestCase('fatal_inside_an_error_mode_window_between_tasks_spares_the_next_task', 'fibers');

$tag = 'between-' . bin2hex(random_bytes(4));

// Kept parked across the tasks below: with nothing in flight the pool may reset
// the thread between tasks, which would hide what the fatal left.
$sleeper = oxphp_async(static function (): void {
    oxphp_sleep(1.0);
});
$dropper = oxphp_async(static function () use ($tag): void {
    // The task runs on another thread, which has classes of its own.
    require_once __DIR__ . '/error_mode_probe.php';
    $fatal = new OxphpErrorModeFatalOnRelease($tag);
    set_error_handler(static function () use ($fatal): bool {
        return true;
    });
    unset($fatal);
    trigger_error("heard-$tag", E_USER_NOTICE);
    restore_error_handler();
});
oxphp_async_await($dropper, 5.0);
// Started after the other task is let go of: the thread lets go of a task before
// it hands the task's result over.
[$fataled, $next] = oxphp_async_await(oxphp_async(static function () use ($tag): array {
    require_once __DIR__ . '/error_mode_probe.php';
    $fataled = OxphpErrorModeProbe::$fataling[$tag] ?? false;

    return [$fataled, OxphpErrorModeProbe::measure('next-task')];
}), 5.0);
oxphp_async_await($sleeper, 5.0);

$t->assertTrue('premise: letting go of the first task fataled inside its open, on this thread', $fataled);
$t->assertSame('the task taken after it: its own handler hears its errors', $next['heard'], ['notice-next-task', 'warning-next-task']);
$t->assertSame('and nothing is thrown in their place', $next['thrown'], 'none');

$t->done();
