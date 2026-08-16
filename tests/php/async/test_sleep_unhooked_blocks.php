<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('sleep_unhooked_blocks', 'async');

// RUNTIME_HOOKS is unset in this profile: native sleep() must remain the
// blocking builtin. A task calling sleep(3) pins its worker inside one C
// call — cancellation lands only when the call returns, so the finally
// marker must NOT appear within 2s, and must appear once the sleep ends.
$marker = sys_get_temp_dir() . '/oxphp_nohook_' . getmypid() . '_' . uniqid('', true);

$task = oxphp_async(function () use ($marker): int {
    try {
        sleep(3);
    } finally {
        file_put_contents($marker, 'done');
    }
    return 0;
});

$timed_out = false;
try {
    oxphp_async_await($task, 0.2);
} catch (\OxPHP\Async\TimeoutException $e) {
    $timed_out = true;
}
$t->assertTrue('await timed out at 200ms', $timed_out);

$deadline = microtime(true) + 2.0;
$appeared_early = false;
while (microtime(true) < $deadline) {
    if (file_exists($marker)) { $appeared_early = true; break; }
    usleep(50000);
}
$t->assertTrue('unhooked sleep pinned the worker (finally NOT within 2s)', !$appeared_early);

$deadline = microtime(true) + 3.0;
while (!file_exists($marker) && microtime(true) < $deadline) {
    usleep(50000);
}
$t->assertTrue('finally ran once the blocking sleep returned', file_exists($marker));

if (file_exists($marker)) {
    unlink($marker);
}
$t->done();
