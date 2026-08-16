<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('await_all_timeout_in_fiber', 'async');

// await_all timing out while SUSPENDED inside a task fiber (composition), so it
// takes the cooperative fiber_await path rather than the top-level blocking
// await. The outer task awaits a fan-out where one member never finishes within
// the inner timeout; the inner await_all must time out and the failure must
// propagate out of the outer task — instead of the outer task hanging until its
// own (much larger) await budget.
$outer = oxphp_async(function (): array {
    $fast = oxphp_async(fn (): int => 1);
    // Cooperative sleep: suspends the fiber so the single worker is free for the
    // inner await_all's 0.2s timer to fire. A blocking sleep would pin the only
    // worker and the timeout could never be honoured.
    $slow = oxphp_async(function (): int {
        oxphp_sleep(10.0);
        return 2;
    });
    return oxphp_async_await_all([$fast, $slow], 0.2);
});

$start = microtime(true);
$threw = false;
$cls = '';
$msg = '';
try {
    oxphp_async_await($outer, 5.0);
} catch (\Throwable $e) {
    $threw = true;
    $cls = get_class($e);
    $msg = $e->getMessage();
}
$elapsed = microtime(true) - $start;

$t->assertTrue('outer await threw (inner await_all timed out in fiber)', $threw);

// Fast failure is the discriminator: it proves the INNER 0.2s timeout fired,
// not the 10s slow member and not the 5s outer budget. A non-timing-out inner
// await_all would instead hang until the outer budget (~5s).
$t->assertLessThan('inner timeout fired fast, not slow-wait or outer budget', $elapsed, 2.0);

// The inner timeout happened *inside* a task we awaited, so the outer task
// rejects and the failure surfaces at our await as AsyncException — a task that
// throws is always reported as a rejection, distinct from a timeout on our own
// await call (which would surface as TimeoutException). The original
// TimeoutException is preserved in the wrapped message as the cause.
$t->assertSame('inner timeout surfaces as a rejected task (AsyncException)', $cls, \OxPHP\Async\AsyncException::class);
$t->assertContains('rejection cause is the inner await_all timeout', $msg, 'TimeoutException');
$t->assertContains('rejection message names the timed-out await_all', $msg, 'timed out');

$t->done();
