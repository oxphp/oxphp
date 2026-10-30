<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('task_driver_window_error_does_not_reach_the_next_task', 'asyncnocache');

// The closure a task ran is released on the async thread after the task ends,
// outside any task, and a destructor that runs then can raise a warning. No
// task is left to own it, so it goes before the next one starts.
//
// A task of this request sleeps on the async thread meanwhile, so the thread
// is never idle in between.
$t->assertSame('OPcache is off', (bool) ini_get('opcache.enable'), false);

$sentinel = oxphp_async(static function (): array {
    $started = microtime(true);
    oxphp_sleep(3.0);

    return [$started, microtime(true)];
});

$mark = sys_get_temp_dir() . '/oxphp-window-error-' . bin2hex(random_bytes(4));
$t->assertSame('the task returns', oxphp_async_await(oxphp_async(static function (string $mark): int {
    static $keep;
    $keep = require __DIR__ . '/fixture_window_error_object.php';

    return 7;
}, $mark)), 7);
$t->assertTrue('the destructor ran once the task ended', file_exists($mark));
@unlink($mark);

[$readAt, $seen] = oxphp_async_await(oxphp_async(static function (): array {
    return [microtime(true), error_get_last()['message'] ?? null];
}));
[$started, $ended] = oxphp_async_await($sentinel);
$t->assertTrue('the async thread is not idle in between', $started < $readAt && $readAt < $ended);
$t->assertNull('the next task finds no error of the release', $seen);

$t->done();
