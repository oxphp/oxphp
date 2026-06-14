<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('cancel_race_timeout', 'async');

// A CPU-bound task inside a fan-out (await_race). On race timeout the dispatch
// path sets each pending task's cancel flag — but a busy-looping fiber never
// returns to its scheduler loop to observe it, so Path A cannot reach it. The
// timeout must ALSO kick the running worker's vm_interrupt cross-thread so the
// fiber is broken out at an opcode boundary and unwound, running its `finally`
// (which writes the marker). Without the kick the fiber spins until RSHUTDOWN
// and the marker is never written within the test window.
$marker = sys_get_temp_dir() . '/oxphp_racekick_' . getmypid() . '_' . uniqid('', true);

$task = oxphp_async(function () use ($marker): int {
    try {
        $x = 0;
        while (true) {            // never yields; JMP backedge checks vm_interrupt
            $x++;
        }
    } finally {
        file_put_contents($marker, 'cancelled');
    }
    // unreachable
    return 0;
});

// Race over the single still-running task with a 200ms budget. It never
// settles, so the race times out — setting the cancel flag and kicking the
// worker that runs the task.
$timed_out = false;
try {
    oxphp_async_await_race([$task], 0.2);
} catch (\OxPHP\Async\TimeoutException $e) {
    $timed_out = true;
}
$t->assertTrue('race await timed out (cancellation trigger fired)', $timed_out);

// The fiber unwinds on the worker thread as the interrupt fires; `finally` runs
// there asynchronously, so poll briefly for the marker (2s < a runaway loop).
$deadline = microtime(true) + 2.0;
while (!file_exists($marker) && microtime(true) < $deadline) {
    usleep(20000);
}

$t->assertTrue(
    'CPU-bound fiber in race fan-out was interrupted and unwound (finally ran)',
    file_exists($marker)
);

if (file_exists($marker)) {
    unlink($marker);
}
$t->done();
