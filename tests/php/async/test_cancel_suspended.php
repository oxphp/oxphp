<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('cancel_suspended', 'async');

// A task that suspends on an inner await, then writes a marker file only if it
// resumes and runs to completion. When the outer await times out, the task is
// cancelled while suspended (Path A) and must unwind before reaching the write.
$marker = sys_get_temp_dir() . '/oxphp_cancel_' . getmypid() . '_' . uniqid('', true);

$task = oxphp_async(function () use ($marker): int {
    // Inner task delays ~700ms so the outer task stays suspended on the await.
    $inner = oxphp_async(function (): int {
        usleep(700000);
        return 1;
    });
    oxphp_async_await($inner);          // outer suspends here until inner finishes
    file_put_contents($marker, 'ran');  // reached only if NOT cancelled
    return 1;
});

// Give up after 100ms — this sets the promise's cancel flag, which the async
// worker uses to cancel the still-suspended task fiber.
$timed_out = false;
try {
    oxphp_async_await($task, 0.1);
} catch (\OxPHP\Async\TimeoutException $e) {
    $timed_out = true;
}
$t->assertTrue('outer await timed out (cancellation trigger fired)', $timed_out);

// Wait well past when the task would have completed naturally (~700ms inner).
usleep(1500000);

$t->assertFalse(
    'suspended task cancelled before its post-await side effect ran',
    file_exists($marker)
);

if (file_exists($marker)) {
    unlink($marker);
}
$t->done();
