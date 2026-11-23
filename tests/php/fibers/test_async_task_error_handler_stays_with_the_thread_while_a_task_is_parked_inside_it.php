<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// Tasks on one async thread share its error handler as requests share a
// worker's. A task parked inside a call of that handler leaves it with the
// thread for a task that runs meanwhile, and finds itself still inside the call
// when it resumes. ASYNC_WORKERS is 1 in this profile, so both tasks run on one
// thread.

$t = new TestCase('async_task_error_handler_stays_with_the_thread_while_a_task_is_parked_inside_it', 'fibers');

$tag = 'task-' . bin2hex(random_bytes(4));

$parker = oxphp_async(static function () use ($tag): array {
    require_once __DIR__ . '/error_mode_probe.php';
    $reentered = 0;
    $handler = static function (int $severity, string $message) use ($tag, &$reentered): bool {
        if ($message === "park-$tag") {
            OxphpErrorModeProbe::$inWindow[$tag] = true;
            // Hooked: parks this task inside its handler.
            usleep(800_000);
            OxphpErrorModeProbe::$inWindow[$tag] = false;
            trigger_error("inside-$tag", E_USER_NOTICE);

            return true;
        }
        if ($message === "inside-$tag") {
            $reentered++;

            return true;
        }

        throw new \ErrorException("parked-handler:$message", 0, $severity);
    };
    set_error_handler($handler);
    $before = microtime(true);
    ob_start();
    trigger_error("park-$tag", E_USER_WARNING);
    ob_end_clean();
    $after = microtime(true);
    restore_error_handler();

    return [$before, $after, $reentered];
});
usleep(200_000);
[$readAt, $premise, $thrown] = oxphp_async_await(oxphp_async(static function () use ($tag): array {
    require_once __DIR__ . '/error_mode_probe.php';
    $readAt = microtime(true);
    $premise = OxphpErrorModeProbe::$inWindow[$tag] ?? false;
    ob_start();
    try {
        trigger_error("probe-$tag", E_USER_WARNING);
        $thrown = 'none';
    } catch (\Throwable $e) {
        $thrown = get_class($e) . ':' . $e->getMessage();
    }
    ob_end_clean();

    return [$readAt, $premise, $thrown];
}), 5.0);
[$before, $after, $reentered] = oxphp_async_await($parker, 5.0);

$t->assertTrue('the sibling runs while the other task is parked inside its handler', $before < $readAt && $readAt < $after);
$t->assertTrue('read by the sibling as it raised', $premise);
$t->assertSame('the sibling\'s warning is heard by the thread\'s handler', $thrown, "ErrorException:parked-handler:probe-$tag");
$t->assertSame('the parked task, resumed inside its handler: an error raised there is not handed back to it', $reentered, 0);

$t->done();
