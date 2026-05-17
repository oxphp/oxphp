<?php
/**
 * Pool — unified timeout contract on acquire():
 *   null=forever, 0.0=try, positive=bounded, INF=forever,
 *   NaN→TypeException, negative→TypeException.
 *
 * Saturation is exercised synchronously (hold the only slot, then
 * try to acquire another). The async-driven "release wakes a parked
 * waiter" path is covered by other tests under the async profile.
 */
header('Content-Type: text/plain');

$pool = new OxPHP\Shared\Pool(
    fn(): object => new stdClass(),
    null,
    1, // maxSize → only one slot
);

// null = forever — bare acquire() succeeds on an empty pool.
$held = $pool->acquire();
if (!$held instanceof OxPHP\Shared\Pool\Handle) { echo "FAIL: acquire() must return Handle\n"; exit; }

// 0.0 = try → acquire on saturated pool throws OperationTimeoutException without blocking.
$start = microtime(true);
$threw = false;
try {
    $pool->acquire(0.0);
} catch (OxPHP\Shared\OperationTimeoutException $e) {
    $threw = true;
}
$elapsed = microtime(true) - $start;
if (!$threw) { echo "FAIL: acquire(0.0) on saturated must throw OperationTimeoutException\n"; exit; }
if ($elapsed >= 0.05) { echo "FAIL: acquire(0.0) must be immediate, elapsed=$elapsed\n"; exit; }

// Positive timeout on saturated pool also throws OperationTimeoutException after the budget.
$start = microtime(true);
$threw = false;
try {
    $pool->acquire(0.05); // 50ms budget
} catch (OxPHP\Shared\OperationTimeoutException $e) {
    $threw = true;
}
$elapsed = microtime(true) - $start;
if (!$threw) { echo "FAIL: acquire(0.05) on saturated must throw OperationTimeoutException\n"; exit; }
if ($elapsed < 0.04) { echo "FAIL: acquire(0.05) must wait at least ~50ms, elapsed=$elapsed\n"; exit; }
if ($elapsed >= 1.0) { echo "FAIL: acquire(0.05) must time out under 1s, elapsed=$elapsed\n"; exit; }

// NaN, negative → TypeException — input validation runs before the wait.
foreach ([NAN, -0.5] as $bad) {
    $caught = null;
    try {
        $pool->acquire($bad);
    } catch (OxPHP\Shared\TypeException $e) {
        $caught = $e;
    }
    if ($caught === null) {
        echo "FAIL: acquire(" . var_export($bad, true) . ") must throw TypeException\n"; exit;
    }
}

// Release the held slot, then re-acquire with INF to confirm the slot
// becomes available immediately (no contention now).
$pool->release($held);
$got = $pool->acquire(INF);
if (!$got instanceof OxPHP\Shared\Pool\Handle) { echo "FAIL: acquire(INF) must return Handle after release\n"; exit; }
$pool->release($got);

echo "OK\n";
