<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('cancel_any_timeout', 'async');

// As test_cancel_race_timeout, but for await_any. On any-timeout the dispatch
// path sets each still-pending task's cancel flag; a busy-looping fiber never
// returns to its scheduler to see it, so the timeout must also kick the running
// worker's vm_interrupt cross-thread to break the fiber out and unwind it,
// running its `finally`. Without the kick the marker is never written in time.
$marker = sys_get_temp_dir() . '/oxphp_anykick_' . getmypid() . '_' . uniqid('', true);

$task = oxphp_async(function () use ($marker): int {
    try {
        $x = 0;
        while (true) {            // never fulfils; JMP backedge checks vm_interrupt
            $x++;
        }
    } finally {
        file_put_contents($marker, 'cancelled');
    }
    // unreachable
    return 0;
});

// Wait for the first task to FULFIL with a 200ms budget. It never does, so
// await_any times out — setting the cancel flag and kicking the worker.
$timed_out = false;
try {
    oxphp_async_await_any([$task], 0.2);
} catch (\OxPHP\Async\TimeoutException $e) {
    $timed_out = true;
}
$t->assertTrue('any await timed out (cancellation trigger fired)', $timed_out);

$deadline = microtime(true) + 2.0;
while (!file_exists($marker) && microtime(true) < $deadline) {
    usleep(20000);
}

$t->assertTrue(
    'CPU-bound fiber in any fan-out was interrupted and unwound (finally ran)',
    file_exists($marker)
);

if (file_exists($marker)) {
    unlink($marker);
}
$t->done();
