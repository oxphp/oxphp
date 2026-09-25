<?php
/**
 * A cancelled async task's cleanup can still take a lock. Once the task has
 * been unwound, its cancel flag stays set for the rest of the task — that is
 * not a cancellation still to come, so a withLock() in its `finally` must wait
 * for the lock and run, not give up on it.
 *
 * An async task holds the mutex for 1.5 s. A second, CPU-bound task is
 * cancelled when this request's await gives up on it after 0.2 s; its
 * `finally` then waits on the held mutex and, once acquired, writes a marker.
 *
 * Needs ASYNC_WORKERS >= 2: the holder and the cancelled task
 * have to run on different threads.
 */
use OxPHP\Shared\Mutex;

header('Content-Type: text/plain');

$marker = sys_get_temp_dir() . '/oxphp_mutex_cleanup_' . getmypid() . '_' . uniqid('', true);

$m = new Mutex(initial: 0);
$holder = oxphp_async(function () use ($m) {
    $m->withLock(function (&$v) {
        usleep(1_500_000);
    });
});
usleep(200_000); // let the holder acquire

$busy = oxphp_async(function () use ($m, $marker) {
    try {
        $x = 0;
        while (true) { // never yields; the cancel reaches it at a loop backedge
            $x++;
        }
    } finally {
        $m->withLock(function (&$v) use ($marker) {
            file_put_contents($marker, 'acquired');
        });
    }
});

try {
    oxphp_async_await($busy, 0.2);
} catch (\OxPHP\Async\TimeoutException $e) {
}

// The holder lets go at ~1.7 s; the cleanup then acquires.
$deadline = microtime(true) + 4.0;
while (!is_file($marker) && microtime(true) < $deadline) {
    usleep(50_000);
}
$acquired = is_file($marker);
@unlink($marker);
if (!$acquired) {
    echo "FAIL: the cancelled task's cleanup gave up on the lock instead of waiting for it\n";
    return;
}

echo "OK\n";
