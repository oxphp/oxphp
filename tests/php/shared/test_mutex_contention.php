<?php
/**
 * tryWithLock against a lock held by another fiber throws ContentionException.
 *
 * Same-thread re-entry is treated as DeadlockException (deterministic
 * deadlock), so real contention requires a second execution context —
 * this test dispatches the holder via oxphp_async() and requires
 * worker mode + ASYNC_WORKERS >= 2.
 */
use OxPHP\Shared\{Mutex, ContentionException};

header('Content-Type: text/plain');

if (!oxphp_is_worker()) {
    echo "OK skip: not worker mode\n";
    return;
}

$m = new Mutex(initial: 0);

// Holder fiber acquires the lock and parks for 300ms.
$held = oxphp_async(function () use ($m) {
    $m->withLock(function (&$v) {
        usleep(300_000); // 300ms — long enough for the contender to observe
    });
});

usleep(50_000); // 50ms — let the holder acquire before we attempt tryWithLock.

$threw = false;
try {
    $m->tryWithLock(fn (&$x) => $x++);
} catch (ContentionException) {
    $threw = true;
}
if (!$threw) { echo "FAIL: tryWithLock against held lock must throw ContentionException\n"; exit; }

oxphp_async_await($held);

echo "OK\n";
