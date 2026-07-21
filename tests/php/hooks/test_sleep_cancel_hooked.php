<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('sleep_cancel_hooked', 'hooks');

// With RUNTIME_HOOKS=1 the native sleep() inside a task fiber must behave
// exactly like oxphp_sleep(): suspend the fiber, and when the awaiter gives
// up and the task is cancelled, force-resume + unwind well before the full
// sleep elapses. Without the hook, native sleep() pins the async worker
// thread inside one C call for the full 5s and the finally marker cannot
// appear within the 2s budget.
$marker = sys_get_temp_dir() . '/oxphp_hookcancel_' . getmypid() . '_' . uniqid('', true);

$task = oxphp_async(function () use ($marker): int {
    try {
        sleep(5);                 // native builtin — hooked to cooperative suspend
    } finally {
        file_put_contents($marker, 'cancelled');
    }
    return 0;
});

$timed_out = false;
try {
    oxphp_async_await($task, 0.2);
} catch (\OxPHP\Async\TimeoutException $e) {
    $timed_out = true;
}
$t->assertTrue('outer await timed out (cancellation trigger fired)', $timed_out);

$deadline = microtime(true) + 2.0;
while (!file_exists($marker) && microtime(true) < $deadline) {
    usleep(20000);
}

$t->assertTrue(
    'hooked native sleep was cancelled and unwound before 5s elapsed (finally ran)',
    file_exists($marker)
);

if (file_exists($marker)) {
    unlink($marker);
}
$t->done();
