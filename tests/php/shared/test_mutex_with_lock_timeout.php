<?php
/**
 * withLockTimeout against a lock held by another fiber throws
 * OperationTimeoutException once the deadline expires.
 *
 * Same-thread re-entry is treated as DeadlockException (deterministic
 * deadlock), so real contention requires a second execution context —
 * this test dispatches the holder via oxphp_async() and requires
 * worker mode + ASYNC_WORKERS >= 2. The $ms=0 TypeException check is
 * a pure single-thread input-validation assertion.
 */
use OxPHP\Shared\{Mutex, OperationTimeoutException};

header('Content-Type: text/plain');

// $ms = 0 → TypeException (input validation; works in any mode).
$badMs = false;
try { (new Mutex(0))->withLockTimeout(fn (&$x) => $x, 0); }
catch (OxPHP\Shared\TypeException) { $badMs = true; }
if (!$badMs) { echo "FAIL: ms=0 must throw TypeException\n"; exit; }

if (!oxphp_is_worker()) {
    echo "OK skip: not worker mode\n";
    return;
}

$m = new Mutex(initial: 0);

// Holder fiber acquires the lock and parks for 500ms.
$held = oxphp_async(function () use ($m) {
    $m->withLock(function (&$v) {
        usleep(500_000); // 500ms — longer than our timeout budget
    });
});

usleep(50_000); // 50ms — let the holder acquire.

$threw = false;
try {
    $m->withLockTimeout(fn (&$x) => $x++, ms: 100);
} catch (OperationTimeoutException) {
    $threw = true;
}
if (!$threw) { echo "FAIL: withLockTimeout must throw OperationTimeoutException\n"; exit; }

oxphp_async_await($held);

echo "OK\n";
