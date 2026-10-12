<?php

declare(strict_types=1);

// Fixture for the orphan-cancel tests: dispatch `orphans` CPU-bound async tasks
// and return without awaiting any of them, so the end of this request is what
// has to cancel them. Each task spins in a loop that never suspends — only an
// interrupt raised on its pool thread can reach it — and records how it ended:
// `<marker>.result` holds the exception that unwound it, or "ran to its end"
// when nothing did and the loop ran out on its own after 6 s. `<marker>.started`
// is written as a task starts running.
//
// The request waits until the first task is running before it returns: a task
// still queued when the request ends finds its cancel flag set when it is picked
// up and never starts, which exercises nothing.
$id = preg_replace('/[^A-Za-z0-9]/', '', (string) ($_GET['id'] ?? ''));
$orphans = max(1, (int) ($_GET['orphans'] ?? 1));
$base = sys_get_temp_dir() . "/oxphp_orphan_{$id}";

$spin = static function (string $marker): void {
    file_put_contents("$marker.started", '1');
    $t = microtime(true);
    try {
        while (microtime(true) - $t < 6) {
            // Never suspends; the call and the loop's jump are where the VM
            // checks for an interrupt.
        }
        file_put_contents("$marker.result", 'ran to its end');
    } catch (\Throwable $e) {
        file_put_contents("$marker.result", get_class($e) . ': ' . $e->getMessage());
        throw $e;
    }
};

for ($i = 0; $i < $orphans; $i++) {
    oxphp_async($spin, "{$base}_{$i}");
    if ($i === 0) {
        $deadline = microtime(true) + 2.0;
        while (!file_exists("{$base}_0.started") && microtime(true) < $deadline) {
            usleep(5000);
            clearstatcache();
        }
        if (!file_exists("{$base}_0.started")) {
            echo 'first-orphan-never-started';
            return;
        }
    }
}

echo 'orphans-dispatched';
