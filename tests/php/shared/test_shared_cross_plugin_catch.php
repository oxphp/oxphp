<?php
/**
 * A single `catch (AsyncException)` must sweep Shared\* and Async\* errors
 * that derive from AsyncException — OperationTimeoutException,
 * ContentionException, and DeadlockException all extend AsyncException.
 *
 * For the Mutex assertions below the same-thread re-entry path raises
 * DeadlockException (it is treated as a deterministic deadlock, not as
 * contention/timeout), which still derives from AsyncException so the
 * single catch arm covers it. Channel timeouts return RecvResult::Timeout
 * instead of throwing.
 */
use OxPHP\Shared\{Mutex, Channel};
use OxPHP\Async\AsyncException;

header('Content-Type: text/plain');

// Mutex error from withLockTimeout caught as AsyncException.
$m = new Mutex(0);
$caughtMutex = false;
$m->withLock(function (&$v) use ($m, &$caughtMutex) {
    try { $m->withLockTimeout(fn (&$x) => $x, ms: 50); }
    catch (AsyncException) { $caughtMutex = true; }
});
if (!$caughtMutex) { echo "FAIL: Mutex withLockTimeout error not caught as AsyncException\n"; exit; }

// Channel recvTimeout maps to RecvResult::Timeout — does NOT throw.
$ch = new Channel(2);
$r = $ch->recvTimeout(20);
if (!$r->isTimeout()) { echo "FAIL: Channel recvTimeout should be RecvResult::Timeout (not throw)\n"; exit; }

// Mutex error from tryWithLock during a held lock caught as AsyncException.
$caughtCont = false;
$m2 = new Mutex(0);
$m2->withLock(function (&$v) use ($m2, &$caughtCont) {
    try { $m2->tryWithLock(fn (&$x) => $x); }
    catch (AsyncException) { $caughtCont = true; }
});
if (!$caughtCont) { echo "FAIL: Mutex tryWithLock error not caught as AsyncException\n"; exit; }

echo "OK\n";
