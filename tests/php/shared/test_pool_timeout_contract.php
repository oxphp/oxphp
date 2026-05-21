<?php
/**
 * Pool — int-millisecond timeout trichotomy on the acquire family:
 *   acquire()              → wait forever
 *   tryAcquire()           → non-blocking; null when saturated
 *   acquireTimeout(int $ms) → bounded; $ms > 0 or TypeException
 *
 * Saturation is exercised synchronously (hold the only slot, then try
 * to acquire another). The async-driven "release wakes a parked waiter"
 * path is covered by other tests under the async profile.
 */
header('Content-Type: text/plain');

$pool = new OxPHP\Shared\Pool(
    fn(): object => new stdClass(),
    null,
    1, // maxSize → only one slot
);

// Bare acquire() returns a Handle on an empty pool (waits forever).
$held = $pool->acquire();
if (!$held instanceof OxPHP\Shared\Pool\Handle) { echo "FAIL: acquire() must return Handle\n"; exit; }

// tryAcquire() on a saturated pool returns null, immediately.
$start = microtime(true);
$none = $pool->tryAcquire();
$elapsed = microtime(true) - $start;
if ($none !== null) { echo "FAIL: tryAcquire on saturated must return null\n"; exit; }
if ($elapsed >= 0.05) { echo "FAIL: tryAcquire must be immediate, elapsed=$elapsed\n"; exit; }

// acquireTimeout(int) on a saturated pool throws OperationTimeoutException
// after roughly the budget.
$start = microtime(true);
$threw = false;
try {
    $pool->acquireTimeout(50); // 50ms budget
} catch (OxPHP\Shared\OperationTimeoutException $e) {
    $threw = true;
}
$elapsed = microtime(true) - $start;
if (!$threw) { echo "FAIL: acquireTimeout(50) on saturated must throw OperationTimeoutException\n"; exit; }
if ($elapsed < 0.04) { echo "FAIL: acquireTimeout(50) must wait ~50ms, elapsed=$elapsed\n"; exit; }
if ($elapsed >= 1.0) { echo "FAIL: acquireTimeout(50) must time out under 1s, elapsed=$elapsed\n"; exit; }

// Invalid budgets (<= 0) are TypeException — validation runs before the wait.
foreach ([0, -1, -100] as $bad) {
    $caught = null;
    try {
        $pool->acquireTimeout($bad);
    } catch (OxPHP\Shared\TypeException $e) {
        $caught = $e;
    }
    if ($caught === null) {
        echo "FAIL: acquireTimeout($bad) must throw TypeException\n"; exit;
    }
}

// After releasing the held slot, bare acquire() succeeds immediately.
$held->release();
$got = $pool->acquire();
if (!$got instanceof OxPHP\Shared\Pool\Handle) { echo "FAIL: acquire() after release must return Handle\n"; exit; }
$got->release();

echo "OK\n";
