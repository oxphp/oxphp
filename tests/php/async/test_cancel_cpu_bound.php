<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('cancel_cpu_bound', 'async');

// A CPU-bound task that never suspends cooperatively (busy loop). Path A cannot
// reach it: the single async worker is pinned inside the fiber, so its scheduler
// loop never runs to poll the cancel flag. Path B must break in via a
// cross-thread Zend VM interrupt at a loop backedge, throwing into the fiber so
// it unwinds — running the `finally`, which writes the marker. Without Path B
// the fiber spins forever and the marker is never written.
$marker = sys_get_temp_dir() . '/oxphp_pathb_' . getmypid() . '_' . uniqid('', true);

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

// Give up after 200ms — sets the promise cancel flag and kicks the worker's
// vm_interrupt so the still-running fiber is interrupted.
$timed_out = false;
try {
    oxphp_async_await($task, 0.2);
} catch (\OxPHP\Async\TimeoutException $e) {
    $timed_out = true;
}
$t->assertTrue('outer await timed out (cancellation trigger fired)', $timed_out);

// The fiber unwinds on the worker thread as the interrupt fires; `finally` runs
// there asynchronously, so poll briefly for the marker.
$deadline = microtime(true) + 2.0;
while (!file_exists($marker) && microtime(true) < $deadline) {
    usleep(20000);
}

$t->assertTrue(
    'CPU-bound fiber was interrupted and unwound (finally ran)',
    file_exists($marker)
);

if (file_exists($marker)) {
    unlink($marker);
}
$t->done();
