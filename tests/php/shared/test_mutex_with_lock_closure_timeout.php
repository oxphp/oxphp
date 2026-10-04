<?php
/**
 * A request whose max_execution_time runs out inside a withLock() closure
 * leaves the mutex unlocked: the timeout ends the request (504) from inside
 * the closure, and the lock the closure ran under is released on the way out.
 *
 * trigger: allows itself 1 s, takes the mutex with withLock() and spins inside
 * the closure past that, after writing to the state. The timeout ends it (504).
 *
 * check: takes the same mutex with tryWithLock(), from the thread the check
 * runs on and from an async task, and reads the state back. The trigger's
 * write is not kept — its closure never finished — so both read 'clean'. One
 * of the two is on another thread than the trigger was, whichever thread the
 * check lands on: before the fix that one saw ContentionException, and the
 * trigger's own thread saw DeadlockException.
 *
 * Needs worker mode + ASYNC_WORKERS >= 1.
 */
use OxPHP\Shared\Mutex;
use OxPHP\Shared\Registry;

header('Content-Type: text/plain');

$key = 'mutex-with-lock-closure-timeout';
$action = $_GET['action'] ?? 'trigger';

if ($action === 'trigger') {
    // A fresh mutex per run: a rerun against the same server must not start
    // from what the last one left.
    Registry::remove($key);
    $m = Registry::mutex($key, static fn () => new Mutex('clean'));

    set_time_limit(1);
    $m->withLock(static function (&$v) {
        $v = 'dirty';
        $end = microtime(true) + 3.0;
        while (microtime(true) < $end) {
        }
    });

    echo "FAIL: withLock returned past max_execution_time\n";
    return;
}

// action=check — runs right after the trigger's response.
$m = Registry::mutex($key, static fn () => new Mutex('not-created-by-the-trigger'));

try {
    $here = $m->tryWithLock(static fn (&$v) => $v);
} catch (\Throwable $e) {
    $here = get_class($e) . ': ' . $e->getMessage();
}

$there = oxphp_async_await(oxphp_async(static function () use ($m) {
    try {
        return $m->tryWithLock(static fn (&$v) => $v);
    } catch (\Throwable $e) {
        return get_class($e) . ': ' . $e->getMessage();
    }
}));

if ($here !== 'clean') {
    echo 'FAIL: tryWithLock on the check thread got ', var_export($here, true), "\n";
    return;
}
if ($there !== 'clean') {
    echo 'FAIL: tryWithLock on an async thread got ', var_export($there, true), "\n";
    return;
}

echo "OK\n";
