<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('cancel_sleeping', 'async');

// A task parked in a cooperative oxphp_sleep(). Unlike the CPU-bound case, the
// fiber IS suspended, so the worker's scheduler loop runs and can see the cancel
// flag. But the SLEEP suspend point only resumes on timer expiry — so without
// the cancel-aware force-resume the fiber sleeps the full 5s before its
// `finally` runs. The awaiter gives up after 200ms, which must cancel the
// sleeping fiber: the scheduler force-resumes it, the sleep returns "cancelled",
// the task throws, and `finally` writes the marker well before the 5s elapses.
$marker = sys_get_temp_dir() . '/oxphp_sleepcancel_' . getmypid() . '_' . uniqid('', true);

$task = oxphp_async(function () use ($marker): int {
    try {
        oxphp_sleep(5);           // cooperative suspend; resumed on timer OR cancel
    } finally {
        file_put_contents($marker, 'cancelled');
    }
    return 0;
});

// Give up after 200ms — sets the promise cancel flag, which the driver
// propagates to the sleeping fiber so the scheduler force-resumes it.
$timed_out = false;
try {
    oxphp_async_await($task, 0.2);
} catch (\OxPHP\Async\TimeoutException $e) {
    $timed_out = true;
}
$t->assertTrue('outer await timed out (cancellation trigger fired)', $timed_out);

// The fiber unwinds on the worker thread as the cancel force-resumes it;
// `finally` runs there asynchronously, so poll briefly for the marker. The
// 2s budget is comfortably below the 5s sleep — if the sleep ran to completion
// the marker would not appear in time.
$deadline = microtime(true) + 2.0;
while (!file_exists($marker) && microtime(true) < $deadline) {
    usleep(20000);
}

$t->assertTrue(
    'sleeping fiber was cancelled and unwound before its sleep elapsed (finally ran)',
    file_exists($marker)
);

if (file_exists($marker)) {
    unlink($marker);
}
$t->done();
