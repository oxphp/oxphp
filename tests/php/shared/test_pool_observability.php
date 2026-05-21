<?php
/**
 * Shared\Pool observability smoke.
 *
 * Drives end-to-end behaviour required by the observability contract:
 *
 *   1. Every acquire result (ok / timeout / closed / saturated)
 *      increments `oxphp_shared_pool_acquire_total{result=...}`. A
 *      non-blocking tryAcquire miss counts as `saturated`, not `timeout`.
 *   2. A user-driven `$pool->evict()` increments
 *      `oxphp_shared_pool_evicted_total{reason="evict"}`.
 *   3. The four gauges size / in_use / idle / waiting appear
 *      for every live Pool with the correct label.
 *   4. The wait histogram emits cumulative buckets + sum + count
 *      per Pool.
 *
 * Fetches the Prometheus exposition text from the container's
 * internal server (127.0.0.1:9090/metrics) so the assertions
 * exercise the full collector path, not just getter plumbing.
 */

header('Content-Type: text/plain');

$pool = new OxPHP\Shared\Pool(
    fn(): object => new stdClass(),
    null,
    2,      // maxSize
    50,     // idleTimeoutMs = 50ms
);
$id = $pool->id();

// Two successful acquires, budget now full → acquire_total{ok} += 2.
$h1 = $pool->acquire();
$h2 = $pool->acquire();

// Third acquire with a short timeout must hit the wait-for-release
// path and return OperationTimeoutException → acquire_total{timeout} += 1.
$caught_timeout = false;
try {
    $pool->acquireTimeout(50);
} catch (\OxPHP\Shared\OperationTimeoutException $e) {
    $caught_timeout = true;
}
if (!$caught_timeout) { echo "FAIL: expected OperationTimeoutException\n"; exit; }

// Non-blocking acquire on the still-saturated pool returns null and must
// count as `saturated`, NOT `timeout` — no wait elapsed, so it must also
// leave the wait histogram count untouched (stays 4, asserted below).
$miss = $pool->tryAcquire();
if ($miss !== null) { echo "FAIL: tryAcquire on full pool must return null\n"; exit; }

$h1->release();
$h2->release();

// One stale slot (after releases both are in idle; sleep past
// idleTimeout) + explicit $pool->evict() → evicted_total{evict} += 1
// (only the front slot is stale; evict stops at the first fresh
// entry — but both share the same last_active checkpoint so they
// go together). Use exactly one release → sleep → evict to pin the
// expected count to 1.
$h3 = $pool->acquire();     // reuse idle slot, bumping ok → 3
$h3->release();             // back to idle

$evicted = $pool->evict();
// evict() force-evicts all idle slots now, regardless of age.
if ($evicted < 1) { echo "FAIL: evict returned $evicted, expected ≥ 1\n"; exit; }

// Scrape /metrics from the internal server.
$metrics = @file_get_contents('http://127.0.0.1:9090/metrics');
if ($metrics === false) { echo "FAIL: /metrics fetch failed\n"; exit; }

// All seven metric series must appear for our pool id.
$must_contain = [
    "oxphp_shared_pool_size{pool_id=\"{$id}\"}",
    "oxphp_shared_pool_in_use{pool_id=\"{$id}\"}",
    "oxphp_shared_pool_idle{pool_id=\"{$id}\"}",
    "oxphp_shared_pool_waiting{pool_id=\"{$id}\"}",
    "oxphp_shared_pool_acquire_total{pool_id=\"{$id}\",result=\"ok\"} 3",
    "oxphp_shared_pool_acquire_total{pool_id=\"{$id}\",result=\"timeout\"} 1",
    "oxphp_shared_pool_acquire_total{pool_id=\"{$id}\",result=\"closed\"} 0",
    // tryAcquire on the full pool counts here, not under timeout.
    "oxphp_shared_pool_acquire_total{pool_id=\"{$id}\",result=\"saturated\"} 1",
    // 4 acquire observations (3 ok + 1 timeout) = wait histogram count.
    // The saturated tryAcquire miss is excluded — it never waited.
    "oxphp_shared_pool_wait_seconds_count{pool_id=\"{$id}\"} 4",
    // +Inf bucket must equal total count.
    "oxphp_shared_pool_wait_seconds_bucket{pool_id=\"{$id}\",le=\"+Inf\"} 4",
    "oxphp_shared_pool_evicted_total{pool_id=\"{$id}\",reason=\"idle_timeout\"} 0",
    "oxphp_shared_pool_evicted_total{pool_id=\"{$id}\",reason=\"shutdown\"} 0",
];

foreach ($must_contain as $needle) {
    if (strpos($metrics, $needle) === false) {
        echo "FAIL: missing '{$needle}'\n";
        exit;
    }
}

// evicted{reason=evict} >= 1 (1 or 2 depending on how the two idle
// slots' last_active compare at measurement).
if (preg_match(
    '/oxphp_shared_pool_evicted_total\{pool_id="' . preg_quote((string) $id, '/') .
        '",reason="evict"\}\s+(\d+)/',
    $metrics,
    $m,
) !== 1) {
    echo "FAIL: evict counter line missing\n";
    exit;
}
if ((int) $m[1] < 1) {
    echo "FAIL: evict counter = " . $m[1] . "\n";
    exit;
}

// /__ox_shared/entry JSON shape check.
$entry_json = @file_get_contents("http://127.0.0.1:9090/__ox_shared/entry?id={$id}");
if ($entry_json === false) { echo "FAIL: /__ox_shared/entry fetch failed\n"; exit; }
$entry = json_decode($entry_json, true);
if (!is_array($entry)) { echo "FAIL: entry JSON decode failed\n"; exit; }
if (($entry['type'] ?? null) !== 'Pool') { echo "FAIL: wrong type in entry JSON\n"; exit; }
$ts = $entry['type_specific'] ?? null;
if (!is_array($ts)) { echo "FAIL: type_specific missing\n"; exit; }
foreach (['max_size', 'size', 'in_use', 'idle', 'waiting', 'idle_by_thread', 'rebalance_strategy'] as $k) {
    if (!array_key_exists($k, $ts)) {
        echo "FAIL: type_specific missing '$k'\n";
        exit;
    }
}
if ($ts['rebalance_strategy'] !== 'strict') { echo "FAIL: rebalance_strategy\n"; exit; }
if ($ts['max_size'] !== 2) { echo "FAIL: max_size\n"; exit; }

echo "OK\n";
