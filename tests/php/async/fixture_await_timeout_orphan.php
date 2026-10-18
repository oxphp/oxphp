<?php

declare(strict_types=1);

// Fixture for the single-await-timeout drain test. Dispatch one CPU-bound task,
// await it with a short timeout so the await gives up, catch the
// TimeoutException and return. The task is a static closure (no bound `$this`),
// so this exercises the plain single-await path, not the bound-closure path.
//
// `<marker>.started` is written as the task starts. The task spins without
// suspending until the await-timeout cancel interrupts it, then keeps working
// for `?linger=` more seconds (one by default) — so the end of the request has a
// live task to wait for — and records `<marker>.ended_at` (server wall clock) in
// `finally` when it actually stops. The request end waits for that task, up to
// the drain budget, before the response is sent.
$id = preg_replace('/[^A-Za-z0-9]/', '', (string) ($_GET['id'] ?? ''));
$base = sys_get_temp_dir() . "/oxphp_awaittmo_{$id}";
$linger = min(15.0, max(0.0, (float) ($_GET['linger'] ?? 1)));

$task = static function () use ($base, $linger): void {
    file_put_contents("$base.started", '1');
    $start = microtime(true);
    try {
        while (microtime(true) - $start < 20.0) {
            // CPU-bound; only the await-timeout cancel interrupts this.
        }
    } catch (\Throwable) {
        // Cancelled. Keep working so the request end has to wait.
        $t = microtime(true);
        while (microtime(true) - $t < $linger) {
        }
    } finally {
        file_put_contents("$base.ended_at", (string) microtime(true));
    }
};

$p = oxphp_async($task);
try {
    oxphp_async_await($p, 0.2);
    echo 'await-returned';
} catch (\OxPHP\Async\TimeoutException $e) {
    echo 'await-timed-out';
}
