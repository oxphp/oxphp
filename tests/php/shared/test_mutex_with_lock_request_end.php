<?php
/**
 * withLock() waiting on a lock held elsewhere is left when the request runs
 * out of max_execution_time, instead of holding the worker thread until the
 * holder lets go — which, in a lock-order cycle, it never does.
 *
 * trigger: an async task holds the mutex for 6 s; this request allows itself
 * 1 s and then waits on the mutex with withLock(). The timeout ends it (504).
 *
 * check: reads when the trigger started waiting and asserts the trigger was
 * over well before the holder let go. The 504 alone does not tell the two
 * apart: the timer fires during the wait either way, and a wait that is not
 * left still ends in the same fatal — just once the lock is acquired, 6 s in.
 *
 * Needs worker mode + ASYNC_WORKERS >= 2: same-thread re-entry is a
 * DeadlockException, so the holder has to be another thread.
 */
use OxPHP\Shared\Mutex;

header('Content-Type: text/plain');

$marker = '/tmp/oxphp-mutex-with-lock-request-end.t0';
$action = $_GET['action'] ?? 'trigger';

if ($action === 'trigger') {
    @unlink($marker);

    $m = new Mutex(initial: 0);
    $holder = oxphp_async(function () use ($m) {
        $m->withLock(function (&$v) {
            usleep(6_000_000);
        });
    });
    usleep(200_000); // let the holder acquire

    set_time_limit(1);
    file_put_contents($marker, (string) microtime(true));
    $m->withLock(fn (&$v) => $v++);

    echo "FAIL: withLock returned past max_execution_time\n";
    return;
}

// action=check — runs right after the trigger's response.
$raw = @file_get_contents($marker);
@unlink($marker);
if ($raw === false || (float) $raw <= 0) {
    echo "FAIL: trigger never reached withLock (no marker)\n";
    return;
}
$elapsed = microtime(true) - (float) $raw;
if ($elapsed >= 3.0) {
    printf("FAIL: the waiting request lasted %.1fs — withLock held it until the lock was released, past its 1s max_execution_time\n", $elapsed);
    return;
}

echo "OK\n";
