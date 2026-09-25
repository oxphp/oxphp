<?php
/**
 * A client hanging up does not end a withLock() wait. In worker mode an
 * ordinary request whose client left runs on to its end, so the wait must go
 * on and acquire the lock once the holder lets go — the same as a request
 * whose client stayed.
 *
 * trigger: an async task holds the mutex for 1.5 s; this request waits on it
 * with withLock(). The runner hangs up after 0.5 s. Once acquired, the closure
 * writes a marker.
 *
 * check: waits past the holder's release and asserts the marker exists.
 *
 * Needs worker mode + ASYNC_WORKERS >= 2 (the holder has to be another thread).
 */
use OxPHP\Shared\Mutex;

header('Content-Type: text/plain');

$marker = '/tmp/oxphp-mutex-with-lock-client-abort.acquired';
$action = $_GET['action'] ?? 'trigger';

if ($action === 'trigger') {
    @unlink($marker);

    $m = new Mutex(initial: 0);
    $holder = oxphp_async(function () use ($m) {
        $m->withLock(function (&$v) {
            usleep(1_500_000);
        });
    });
    usleep(200_000); // let the holder acquire

    $m->withLock(function (&$v) use ($marker) {
        file_put_contents($marker, 'acquired');
    });
    echo "done\n";
    return;
}

// action=check — the runner hung up at 0.5 s; the holder lets go at ~1.7 s.
$deadline = microtime(true) + 5.0;
while (!is_file($marker) && microtime(true) < $deadline) {
    usleep(100_000);
}
$acquired = is_file($marker);
@unlink($marker);
if (!$acquired) {
    echo "FAIL: withLock was left when the client hung up; it must wait on and acquire\n";
    return;
}

echo "OK\n";
