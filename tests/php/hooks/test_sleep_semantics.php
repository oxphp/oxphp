<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('sleep_semantics', 'hooks');

// ── No-fiber fallback (traditional-mode request context): the hook must
// delegate to the original native handlers, byte-identical behavior. ──

$t0 = microtime(true);
$ret = sleep(1);
$el = microtime(true) - $t0;
$t->assertTrue('fallback sleep(1) returned int 0', $ret === 0);
$t->assertTrue('fallback sleep(1) actually slept (elapsed >= 1.0s)', $el >= 1.0);

$t0 = microtime(true);
$ret = usleep(100000);
$el = microtime(true) - $t0;
$t->assertTrue('fallback usleep(100000) returned null', $ret === null);
$t->assertTrue('fallback usleep actually slept (elapsed >= 0.1s)', $el >= 0.1);

$t->assertThrows('fallback sleep(-1) throws ValueError', fn () => sleep(-1), \ValueError::class);
$t->assertThrows('fallback usleep(-1) throws ValueError', fn () => usleep(-1), \ValueError::class);

// ── Hooked path (task fiber): same observable contract, but suspending. ──

$task = oxphp_async(function (): array {
    $t0 = microtime(true);
    $ret = sleep(1);
    $sleepEl = microtime(true) - $t0;

    $t0 = microtime(true);
    usleep(200000);
    $usleepEl = microtime(true) - $t0;

    $zeroRet = sleep(0);

    return [$ret, $sleepEl, $usleepEl, $zeroRet];
});
[$ret, $sleepEl, $usleepEl, $zeroRet] = oxphp_async_await($task, 5.0);

$t->assertTrue('hooked sleep(1) returned int 0', $ret === 0);
$t->assertTrue('hooked sleep(1) honored duration (elapsed >= 1.0s)', $sleepEl >= 1.0);
$t->assertTrue('hooked sleep(1) did not oversleep grossly (elapsed < 1.8s)', $sleepEl < 1.8);
$t->assertTrue('hooked usleep(200000) honored duration (elapsed >= 0.2s)', $usleepEl >= 0.2);
$t->assertTrue('hooked sleep(0) returned 0 immediately', $zeroRet === 0);

// Argument errors inside a fiber must surface to the awaiter. The async
// runtime wraps a task's exception, preserving the message — assert on the
// native ValueError message text.
$task = oxphp_async(function (): int {
    sleep(-1);
    return 1;
});
$threw = false;
$msg = '';
try {
    oxphp_async_await($task, 5.0);
} catch (\Throwable $e) {
    $threw = true;
    $msg = $e->getMessage();
}
$t->assertTrue('hooked sleep(-1) in fiber rejects the task', $threw);
$t->assertContains('rejection carries the ValueError message', $msg, 'must be between 0 and');

$t->done();
