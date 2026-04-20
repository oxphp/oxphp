<?php
/**
 * End-to-end perf smoke for Shared\Pool.
 *
 * Measures pool cycle cost (acquire + release) against a direct
 * factory-call baseline across N iterations. The authoritative
 * Rust-side number (~136ns/cycle on an M-series in release) comes
 * from `cargo bench --bench pool_uncontested`. This script is a
 * docker-level regression smoke: it prints the observed overhead
 * and fails only on catastrophic regressions (> 50μs/cycle).
 *
 * The 50μs cap is deliberately generous — docker scheduling and
 * warm-start noise routinely add a few μs on first runs. If this
 * ever trips, either the pool hot path has a real regression or
 * the CI host is badly contended.
 */

header('Content-Type: text/plain');

$pool = new OxPHP\Shared\Pool(
    fn(): object => new stdClass(),
    null,    // destroy
    1,       // maxSize
    300.0,   // idleTimeout
    5.0,     // defaultAcquireTimeout
);

// Warm up both paths — first-call cost is dominated by one-off
// setup (ini cache, opcache, memory arena) that's orthogonal to
// the pool's steady-state overhead we want to measure.
for ($i = 0; $i < 1000; $i++) {
    $h = $pool->acquire();
    $pool->release($h);
}
$factory = fn(): object => new stdClass();
for ($i = 0; $i < 1000; $i++) {
    $r = $factory();
    unset($r);
}

$N = 20_000;

// Pool cycle: acquire + release on same thread, local-hit path.
$t0 = hrtime(true);
for ($i = 0; $i < $N; $i++) {
    $h = $pool->acquire();
    $pool->release($h);
}
$pool_ns = (int) ((hrtime(true) - $t0) / $N);

// Baseline: straight factory call, no pool bookkeeping.
$t0 = hrtime(true);
for ($i = 0; $i < $N; $i++) {
    $r = $factory();
    unset($r);
}
$baseline_ns = (int) ((hrtime(true) - $t0) / $N);

$overhead_ns = $pool_ns - $baseline_ns;
$overhead_us = $overhead_ns / 1000.0;

// Informational — visible in suite logs for tracking over time.
echo sprintf(
    "pool=%dns baseline=%dns overhead=%.2fus\n",
    $pool_ns,
    $baseline_ns,
    $overhead_us,
);

$cap_us = 50.0;
if ($overhead_us > $cap_us) {
    echo sprintf("FAIL: overhead %.2fus exceeds cap %.2fus\n", $overhead_us, $cap_us);
    exit;
}

echo "OK\n";
