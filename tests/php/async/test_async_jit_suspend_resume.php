<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('async_jit_suspend_resume', 'async');

// The async worker loads the JIT-enabled ini (opcache.jit=tracing). A task body
// whose loops far exceed jit_hot_loop (64) gets trace-compiled, so the bulk of
// the work runs as JIT machine code. The task suspends mid-body via oxphp_sleep
// and then does more JIT'd work after resume. A correct accumulated result
// proves the JIT-compiled frame's locals (CV slots on the VM stack) survive the
// fiber context save/restore — the core fiber+JIT interaction of the async pool.
$start = microtime(true);
$result = oxphp_async_await(oxphp_async(function (): int {
    $sum = 0;
    for ($i = 1; $i <= 100000; $i++) {
        $sum += $i;            // hot loop → JIT-traced
    }
    oxphp_sleep(0.05);         // suspend the fiber mid-function
    for ($i = 1; $i <= 100000; $i++) {
        $sum += $i;            // resumes, more JIT'd work on the restored frame
    }
    return $sum;
}));
$elapsed = microtime(true) - $start;

// 2 * sum(1..100000) = 2 * 5000050000. Wrong only if the suspend/resume lost or
// corrupted the JIT'd frame's $sum.
$t->assertSame('JIT-hot task result correct across suspend/resume', $result, 10000100000);

// Confirm the task genuinely suspended (>= the 50 ms sleep), i.e. the resume
// path actually ran rather than the body executing straight through.
$t->assertGreaterThan('task actually suspended (elapsed >= sleep)', $elapsed, 0.045);

$t->done();
