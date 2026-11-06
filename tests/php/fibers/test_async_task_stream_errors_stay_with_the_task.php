<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// Tasks on one async thread take turns at PHP 8.6's stream errors as requests do
// on a worker's: a task that sleeps finds its own stored error when it wakes, and
// a task that runs meanwhile starts with none and leaves none behind. ASYNC_WORKERS
// is 1 in this profile, so both tasks run on one thread.

$t = new TestCase('async_task_stream_errors_stay_with_the_task', 'fibers');

if (!function_exists('stream_last_errors')) {
    $t->assertTrue('nothing is stored before PHP 8.6', true);
    $t->done();
}

$sleeping = oxphp_async(static function (): array {
    fopen('/nonexistent/oxphp-task', 'r', false, stream_context_create(['stream' => ['error_mode' => StreamErrorMode::Silent]]));
    $before = microtime(true);
    oxphp_sleep(1.0);
    $after = microtime(true);
    $own = [];
    foreach (stream_last_errors() as $error) {
        $own[] = $error->message;
    }

    return [$before, $after, $own];
});
usleep(100_000);
[$readAt, $seen] = oxphp_async_await(oxphp_async(static function (): array {
    $readAt = microtime(true);
    $seen = count(stream_last_errors());
    // A directory, so its message is not the one the sleeping task stored.
    fopen('/tmp', 'w', false, stream_context_create(['stream' => ['error_mode' => StreamErrorMode::Silent]]));

    return [$readAt, $seen];
}));
[$before, $after, $own] = oxphp_async_await($sleeping);

$t->assertTrue('the sibling runs while the task sleeps', $before < $readAt && $readAt < $after);
$t->assertSame('the sibling starts with no stored error', $seen, 0);
$t->assertSame('the task reads one stored error when it wakes', count($own), 1);
$t->assertContains('and it is its own', $own[0] ?? '', 'No such file or directory');

$t->done();
