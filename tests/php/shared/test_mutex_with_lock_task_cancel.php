<?php
/**
 * An async task cancelled while withLock() waits on a lock held elsewhere
 * leaves the wait with an AsyncException, instead of holding its pool thread
 * until the holder lets go.
 *
 * An async task holds the mutex for 2.5 s. A second task waits on it with
 * withLock() and is cancelled when this request's await gives up on it after
 * 0.2 s. Its catch records how long the wait lasted and the exception chain.
 *
 * Asserts both halves: the chain carries withLock's own "wait abandoned"
 * message — only the request-end check in the wait produces it; a wait that
 * ran on until it acquired would be cancelled afterwards with a different
 * message — and the wait ended well before the holder let go.
 *
 * Needs ASYNC_WORKERS >= 2: the holder and the waiter have to run on different
 * threads. The holder spins rather than sleeps: a sleep parks its fiber, and a
 * pool thread whose fibers are all parked takes the next task from the queue —
 * the waiter would then re-enter the mutex on the holder's thread, which is a
 * DeadlockException, not a wait.
 */
use OxPHP\Shared\Mutex;

header('Content-Type: text/plain');

$marker = sys_get_temp_dir() . '/oxphp_mutex_task_cancel_' . getmypid() . '_' . uniqid('', true);

$m = new Mutex(initial: 0);
$holder = oxphp_async(function () use ($m) {
    $m->withLock(function (&$v) {
        $end = microtime(true) + 2.5;
        while (microtime(true) < $end) {
        }
    });
});
usleep(200_000); // let the holder acquire

$waiter = oxphp_async(function () use ($m, $marker) {
    $t0 = microtime(true);
    try {
        $m->withLock(function (&$v) {
        });
        $outcome = 'acquired';
    } catch (\Throwable $e) {
        $messages = [];
        for ($p = $e; $p !== null; $p = $p->getPrevious()) {
            $messages[] = get_class($p) . ': ' . $p->getMessage();
        }
        $outcome = implode(' <- ', $messages);
    }
    file_put_contents($marker, sprintf('%.3f|%s', microtime(true) - $t0, $outcome));
});

try {
    oxphp_async_await($waiter, 0.2);
} catch (\OxPHP\Async\TimeoutException $e) {
}

$deadline = microtime(true) + 4.0;
// Poll the content, not the file: it exists before its bytes are written.
while ((($raw = @file_get_contents($marker)) === false || $raw === '') && microtime(true) < $deadline) {
    usleep(50_000);
}
@unlink($marker);
try {
    oxphp_async_await($holder);
} catch (\Throwable $e) {
}

if ($raw === false || $raw === '') {
    echo "FAIL: the cancelled waiter never left withLock\n";
    return;
}
[$elapsed, $outcome] = explode('|', $raw, 2);
if (!str_contains($outcome, 'wait abandoned, the request is being ended')) {
    printf("FAIL: withLock was not left by the cancel (after %ss: %s)\n", $elapsed, $outcome);
    return;
}
if ((float) $elapsed >= 1.5) {
    printf("FAIL: the cancelled wait lasted %ss — until the holder let go, not until the cancel\n", $elapsed);
    return;
}

echo "OK\n";
