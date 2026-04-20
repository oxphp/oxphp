<?php
/**
 * Map perf gate: `setMany(100 items)` ≥ 3× faster than 100× `set`.
 *
 * This honest PHP-layer measurement runs the same workload through
 * both APIs and prints the ratio. A separate Rust-level bench lives
 * in `benches/shared/map_set_many.rs`; the gate explicitly targets the
 * PHP layer because that's where engine dispatch overhead dominates.
 *
 * Run via:
 *   curl -sS "http://127.0.0.1:$PORT/tests/shared/test_map_perf_set_many.php"
 */
header('Content-Type: text/plain');

const N = 100;
const REPEATS = 500;
const WARMUP = 50;
const TRIALS = 5;
const MIN_RATIO = 3.0;

$m = new OxPHP\Shared\Map();

// Workload: a fresh 100-pair batch per iteration. Both APIs get the
// same insert-only path because we `clear()` between iterations.
$pairs = [];
for ($i = 0; $i < N; $i++) { $pairs["k$i"] = $i; }

// Warm up OPcache + JIT for both paths.
for ($w = 0; $w < WARMUP; $w++) {
    $m->clear();
    foreach ($pairs as $k => $v) { $m->set($k, $v); }
    $m->clear();
    $m->setMany($pairs);
}

$best_ratio = 0.0;
$last_single = 0;
$last_batch = 0;

// Take the best of TRIALS runs to filter scheduler noise (PHP workers
// share a host with build/docker jobs in CI and the per-iteration
// timings swing by 2–3× depending on cgroup contention).
for ($t = 0; $t < TRIALS; $t++) {
    $m->clear();
    $t_single = 0;
    for ($r = 0; $r < REPEATS; $r++) {
        $m->clear();
        $t0 = hrtime(true);
        foreach ($pairs as $k => $v) { $m->set($k, $v); }
        $t_single += hrtime(true) - $t0;
    }

    $m->clear();
    $t_batch = 0;
    for ($r = 0; $r < REPEATS; $r++) {
        $m->clear();
        $t0 = hrtime(true);
        $m->setMany($pairs);
        $t_batch += hrtime(true) - $t0;
    }

    $ratio = $t_single / max(1, $t_batch);
    if ($ratio > $best_ratio) {
        $best_ratio = $ratio;
        $last_single = $t_single;
        $last_batch = $t_batch;
    }
}

$per_set_ns = (int)($last_single / (REPEATS * N));
$per_batch_ns = (int)($last_batch / REPEATS);

printf("N=%d repeats=%d trials=%d\n", N, REPEATS, TRIALS);
printf("N*set:   %12d ns total  (%d ns/op per set)\n", (int)$last_single, $per_set_ns);
printf("setMany: %12d ns total  (%d ns/op per batch)\n", (int)$last_batch, $per_batch_ns);
printf("best ratio: %.2fx  (target >= %.1fx)\n", $best_ratio, MIN_RATIO);

if ($best_ratio >= MIN_RATIO) {
    echo "OK\n";
} else {
    echo "FAIL: perf gate unmet\n";
}
