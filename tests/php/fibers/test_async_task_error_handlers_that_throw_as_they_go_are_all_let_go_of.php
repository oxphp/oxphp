<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// The end of a task lets go of the server's references to the handlers it has
// called between tasks, under no PHP frame, where an exception a destructor
// throws becomes a fatal error. Two handlers a task has called and dropped, each
// the last thing holding an object that throws as it goes: both are let go of by
// the end of the task, under the catch that end runs them in, and the thread
// goes on running tasks. A second task on the same thread watches; ASYNC_WORKERS
// is 1 in this profile.

$t = new TestCase('async_task_error_handlers_that_throw_as_they_go_are_all_let_go_of', 'fibers');

$tag = 'task-' . bin2hex(random_bytes(4));

$watcher = oxphp_async(static function () use ($tag): array {
    require_once __DIR__ . '/error_mode_probe.php';
    for ($i = 0; $i < 100 && !(OxphpErrorModeProbe::$inWindow["returned-$tag"] ?? false); $i++) {
        usleep(20_000);
    }
    $returned = OxphpErrorModeProbe::$inWindow["returned-$tag"] ?? false;
    // A slice more, for the end of the other task to run.
    usleep(200_000);

    return [
        $returned,
        OxphpErrorModeProbe::$inWindow["released-1-$tag"] ?? false,
        OxphpErrorModeProbe::$inWindow["released-2-$tag"] ?? false,
    ];
});
$dropper = oxphp_async(static function () use ($tag): void {
    require_once __DIR__ . '/error_mode_probe.php';
    OxphpErrorModeProbe::$inWindow["armed-$tag"] = true;
    foreach ([1, 2] as $n) {
        $marker = new OxphpErrorModeReleaseMarker("released-$n-$tag", "armed-$tag");
        set_error_handler(static function () use ($marker): bool {
            return true;
        });
        unset($marker);
        trigger_error("heard-$n-$tag", E_USER_NOTICE);
        restore_error_handler();
    }
    OxphpErrorModeProbe::$inWindow["returned-$tag"] = true;
});
oxphp_async_await($dropper, 5.0);
[$returned, $released1, $released2] = oxphp_async_await($watcher, 5.0);
$next = oxphp_async_await(oxphp_async(static fn (): string => 'ran'), 5.0);

$t->assertTrue('premise: the watching task saw the other one return', $returned);
$t->assertTrue('what the second handler held was released by the end of the task', $released2);
$t->assertTrue('and what the first one held', $released1);
$t->assertSame('the thread runs the next task', $next, 'ran');

$t->done();
