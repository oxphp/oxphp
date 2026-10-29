<?php

declare(strict_types=1);

// Fixture for the tests of a task that ends without awaiting a task it
// dispatched itself. The request awaits a parent task; the parent dispatches a
// child, waits until the child is running and returns without awaiting it, so
// the end of the parent is what has to cancel the child. The child is created
// inside the parent, so its code is the parent's, not the request's own closure.
//
// The child records, server wall clock: `<marker>.cancelled_at` when the
// cancellation reaches it, after which it keeps working for `?linger=` seconds,
// and `<marker>.ended_at` in `finally` when it actually stops. Nothing but a
// cancellation ends it before 20 s. The parent records `<marker>.parent_done`
// just before it returns.
//
// `?mode=park` parks the child in oxphp_sleep() and has the parent wait for it
// with oxphp_sleep(), so the two can share a single pool thread. `?mode=cpu`
// keeps both on the CPU: the parent never gives up its thread while it waits,
// so the child runs on another one and only an interrupt reaches it.
$id = preg_replace('/[^A-Za-z0-9]/', '', (string) ($_GET['id'] ?? ''));
$base = sys_get_temp_dir() . "/oxphp_orphanchild_{$id}";
$linger = min(15.0, max(0.0, (float) ($_GET['linger'] ?? 1)));
$park = ($_GET['mode'] ?? 'park') === 'park';

$parent = static function (string $base, float $linger, bool $park): string {
    $child = static function () use ($base, $linger, $park): void {
        try {
            // Inside the `try`: the parent returns, and its end cancels this
            // task, as soon as the file exists (see fixture_orphan_cpu_bound.php).
            file_put_contents("$base.started", '1');
            if ($park) {
                oxphp_sleep(20.0);
            } else {
                $t = microtime(true);
                while (microtime(true) - $t < 20.0) {
                }
            }
        } catch (\Throwable) {
            file_put_contents("$base.cancelled_at", (string) microtime(true));
            $t = microtime(true);
            while (microtime(true) - $t < $linger) {
            }
        } finally {
            file_put_contents("$base.ended_at", (string) microtime(true));
        }
    };

    oxphp_async($child);
    $deadline = microtime(true) + 3.0;
    while (!file_exists("$base.started") && microtime(true) < $deadline) {
        if ($park) {
            oxphp_sleep(0.005);
        }
        clearstatcache();
    }
    file_put_contents("$base.parent_done", (string) microtime(true));

    return file_exists("$base.started") ? 'outer-done' : 'child-never-started';
};

echo oxphp_async_await(oxphp_async($parent, $base, $linger, $park), 30.0);
