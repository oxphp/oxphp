<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// Tasks on one async thread take turns at the mode `new SplFileObject()` opens
// in — warnings become exceptions and skip the error handler — as requests do on
// a worker's: a task that parks inside such an open finds its own mode when it
// resumes, and a task that runs meanwhile raises its errors in its own.
// ASYNC_WORKERS is 1 in this profile, so both tasks run on one thread.

$t = new TestCase('async_task_error_mode_stays_with_the_task', 'fibers');

$tag = 'task-' . bin2hex(random_bytes(4));

$opener = oxphp_async(static function () use ($tag): array {
    // The task runs on another thread, which has classes of its own.
    require_once __DIR__ . '/error_mode_probe.php';
    $before = microtime(true);
    $ended = OxphpErrorModeProbe::open("oxehpark://park/800/$tag");

    return [$before, microtime(true), $ended];
});
usleep(200_000);
[$readAt, $premise, $seen] = oxphp_async_await(oxphp_async(static function () use ($tag): array {
    require_once __DIR__ . '/error_mode_probe.php';
    $readAt = microtime(true);
    $premise = OxphpErrorModeProbe::$inWindow[$tag] ?? false;

    return [$readAt, $premise, OxphpErrorModeProbe::measure('sibling')];
}), 5.0);
[$before, $after, $ended] = oxphp_async_await($opener, 5.0);

$t->assertTrue('the sibling runs while the opener is parked inside its open', $before < $readAt && $readAt < $after);
$t->assertTrue('read by the sibling as it measured', $premise);
$t->assertSame('the sibling\'s own handler hears its errors', $seen['heard'], ['notice-sibling', 'warning-sibling']);
$t->assertSame('and nothing is thrown in their place', $seen['thrown'], 'none');
$t->assertSame('the opener resumes in its own mode: its warning is what it throws', $ended, "RuntimeException:after-park-$tag");

$t->done();
