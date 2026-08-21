<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('await_jit_hot_loop_leak', 'async');

// A loop awaiting a large string result gets trace-compiled once its back-edge
// count crosses opcache.jit_hot_loop (default 64; the test image runs
// opcache.jit=tracing). The compiled trace frees the delivered value only if
// the function's declared return type admits refcounted values — so a loop of
// this exact shape (await, then unset, without otherwise reading the value)
// is what detects a wrong declaration: every post-compilation iteration then
// leaks one full result-string allocation (~68 KiB for a 64 KiB payload),
// visible in memory_get_usage() and never reclaimed within the request.
// Reading the value inside the loop (even strlen) must be avoided: it inserts
// a type guard that deoptimizes the trace and hides the leak.

const RESULT_BYTES = 65536;
const HOT_ITERATIONS = 200;

function iter(): void
{
    $id = oxphp_async(static fn(int $n): string => str_repeat('x', $n), RESULT_BYTES);
    $v = oxphp_async_await($id);
    unset($v);
}

// The probe's whole detecting power rests on the trace actually being
// compiled: with the JIT silently off (an unsupported buffer size, an
// extension installing a user opcode handler — zend_jit_check_support()
// downgrades both to a startup warning), a broken build reports the same
// zero growth as a fixed one. Assert the precondition instead of trusting
// the image's ini.
$jit = opcache_get_status(false)['jit'] ?? null;
$t->assertTrue('tracing JIT is active (leak detection precondition)', $jit['on'] ?? false);

// Warmup outside the measured window, mirroring the shape that reproduces the
// leak: a separate short loop first, then the loop that actually gets hot.
for ($i = 0; $i < 10; $i++) {
    iter();
}

$before = memory_get_usage();
for ($i = 0; $i < HOT_ITERATIONS; $i++) {
    iter();
}
$growth = memory_get_usage() - $before;

$t->meta('growth_bytes', $growth);
$t->meta('growth_per_iteration', intdiv($growth, HOT_ITERATIONS));

// Broken builds leak one result allocation per post-compilation iteration
// (~9.5 MB over 200 iterations); a healthy run stays within noise. 1 MB keeps
// a wide margin on both sides.
$t->assertLessThan('hot await loop does not accumulate result allocations', $growth, 1048576);

// Delivery integrity, checked outside the hot loop so the probe above keeps
// its leak-detecting shape.
$id = oxphp_async(static fn(int $n): string => str_repeat('x', $n), RESULT_BYTES);
$t->assertSame('delivered value intact after the hot loop', strlen(oxphp_async_await($id)), RESULT_BYTES);

$t->done();
