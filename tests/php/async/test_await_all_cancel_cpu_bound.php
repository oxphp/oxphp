<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('await_all_cancel_cpu_bound', 'async');

// await_all over two CPU-bound tasks on a multi-worker pool (ASYNC_WORKERS=4),
// so both members run concurrently on separate workers. await_all awaits them
// in order: when the FIRST times out, await_all bails and must cancel the whole
// set — including the SECOND member, which it never got to await. Each task
// writes a marker from its `finally`, which only runs if the busy loop is broken
// by a cross-thread VM interrupt (Path B).
//
//   marker_a — the awaited member; the blocking await itself cancels it, so this
//              appears even without the set-wide strand.
//   marker_b — the abandoned member; appears ONLY if await_all strands the
//              remaining promises on bail. Without that, the task spins
//              unobserved until RSHUTDOWN — past this test's poll window.
$marker_a = sys_get_temp_dir() . '/oxphp_awaitall_a_' . getmypid() . '_' . uniqid('', true);
$marker_b = sys_get_temp_dir() . '/oxphp_awaitall_b_' . getmypid() . '_' . uniqid('', true);

$busy = static function (string $marker): int {
    try {
        $x = 0;
        while (true) {            // never yields; JMP backedge checks vm_interrupt
            $x++;
        }
    } finally {
        file_put_contents($marker, 'cancelled');
    }
    return 0; // unreachable
};

$a = oxphp_async($busy, $marker_a);
$b = oxphp_async($busy, $marker_b);

// Give the whole set 200ms. The first member never completes, so await_all
// times out, cancels it, and must strand the second.
$timed_out = false;
try {
    oxphp_async_await_all([$a, $b], 0.2);
} catch (\OxPHP\Async\TimeoutException $e) {
    $timed_out = true;
}
$t->assertTrue('await_all timed out (cancellation trigger fired)', $timed_out);

// Both fibers unwind on their worker threads as the interrupts fire; the
// `finally` blocks run there asynchronously, so poll briefly for both markers.
$deadline = microtime(true) + 2.0;
while ((!file_exists($marker_a) || !file_exists($marker_b)) && microtime(true) < $deadline) {
    usleep(20000);
}

$t->assertTrue('awaited CPU-bound member was cancelled (finally ran)', file_exists($marker_a));
$t->assertTrue('abandoned CPU-bound member was cancelled too (finally ran)', file_exists($marker_b));

foreach ([$marker_a, $marker_b] as $m) {
    if (file_exists($m)) {
        unlink($m);
    }
}
$t->done();
