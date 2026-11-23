<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// The server keeps its own reference to an error handler for the length of each
// call of it. A handler a task sets, has called and then drops goes by the end
// of that task, and what it holds with it — not when the fiber the task ran in
// is handed another task. A second task on the same thread watches: it is
// parked when the first one starts, so the first runs in a fiber of its own.
// ASYNC_WORKERS is 1 in this profile, so both tasks run on one thread.

$t = new TestCase('async_task_error_handler_dropped_by_a_task_is_let_go_of_by_its_end', 'fibers');

$tag = 'task-' . bin2hex(random_bytes(4));

$watcher = oxphp_async(static function () use ($tag): array {
    require_once __DIR__ . '/error_mode_probe.php';
    for ($i = 0; $i < 100 && !(OxphpErrorModeProbe::$inWindow["returned-$tag"] ?? false); $i++) {
        usleep(20_000);
    }
    $returned = OxphpErrorModeProbe::$inWindow["returned-$tag"] ?? false;
    // A slice more, for the end of the other task to run.
    usleep(200_000);

    return [$returned, OxphpErrorModeProbe::$inWindow["released-$tag"] ?? false];
});
$dropper = oxphp_async(static function () use ($tag): void {
    require_once __DIR__ . '/error_mode_probe.php';
    $marker = new OxphpErrorModeReleaseMarker("released-$tag");
    set_error_handler(static function () use ($marker): bool {
        return true;
    });
    unset($marker);
    trigger_error("heard-$tag", E_USER_NOTICE);
    restore_error_handler();
    OxphpErrorModeProbe::$inWindow["returned-$tag"] = true;
});
oxphp_async_await($dropper, 5.0);
[$returned, $released] = oxphp_async_await($watcher, 5.0);

$t->assertTrue('premise: the watching task saw the other one return', $returned);
$t->assertTrue('what the dropped handler held was released by the end of the task', $released);

$t->done();
