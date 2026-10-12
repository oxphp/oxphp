<?php

declare(strict_types=1);

// Fixture for shared/test_mutex_with_lock_stranded_task_cleanup_survives_request_end.
//
// Strands a CPU-bound task by giving up on it in oxphp_async_await_any(), and
// ends the request while the `finally` that cancellation unwound it into is
// still waiting for a mutex another task holds. Writes `<base>.waiting` when
// that wait starts and `<base>.result` with how it ended, and answers with
// whether that wait was still going as the request ended.

use OxPHP\Shared\Mutex;

header('Content-Type: text/plain');

$id = preg_replace('/[^A-Za-z0-9]/', '', (string) ($_GET['id'] ?? ''));
$base = sys_get_temp_dir() . "/oxphp_stranded_cleanup_{$id}";

$m = new Mutex(initial: 0);
// Holds the mutex for 1.5 s. Nothing awaits it, so the end of this request
// cancels it too; it catches that and holds on, so the lock is still held
// while the stranded task's cleanup waits for it.
oxphp_async(function () use ($m) {
    $m->withLock(function (&$v) {
        $end = microtime(true) + 1.5;
        while (microtime(true) < $end) {
            try {
                while (microtime(true) < $end) {
                }
            } catch (\Throwable $e) {
            }
        }
    });
});
usleep(200_000); // let the holder acquire

$busy = oxphp_async(function () use ($m, $base) {
    try {
        while (true) { // never yields; the cancel reaches it at a loop backedge
        }
    } finally {
        file_put_contents("{$base}.waiting", '1');
        try {
            $m->withLock(function (&$v) use ($base) {
                file_put_contents("{$base}.result", 'acquired');
            });
        } catch (\Throwable $e) {
            file_put_contents("{$base}.result", get_class($e) . ': ' . $e->getMessage());
        }
    }
});

try {
    oxphp_async_await_any([$busy], 0.2);
    echo "await-did-not-time-out\n";
    return;
} catch (\OxPHP\Async\TimeoutException $e) {
}

$deadline = microtime(true) + 1.0;
while (!file_exists("{$base}.waiting") && microtime(true) < $deadline) {
    usleep(5000);
    clearstatcache();
}
if (!file_exists("{$base}.waiting")) {
    echo "cleanup-never-started\n";
} elseif (file_exists("{$base}.result")) {
    echo "cleanup-ended-before-the-request\n";
} else {
    echo "cleanup-still-waiting\n";
}
