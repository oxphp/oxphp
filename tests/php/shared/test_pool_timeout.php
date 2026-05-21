<?php
/**
 * Pool timeout — budget full + nothing to release →
 * OperationTimeoutException with the configured wait.
 *
 * Single-thread request: we hold one handle, then try to acquire
 * a second one past the budget. Since there's no other thread to
 * release, the wait_for_release path must time out cleanly.
 */

header('Content-Type: text/plain');

$pool = new OxPHP\Shared\Pool(
    fn() => new stdClass(),
    null, // destroy
    1,    // maxSize
);

$held = $pool->acquire();  // Takes the only slot.
if ($pool->stats()->inUse() !== 1) { echo "FAIL: inUse=1 expected\n"; exit; }

$start = microtime(true);
$caught = false;
try {
    // 150ms budget — short enough to keep the test fast, long
    // enough that we don't tick below Condvar wake granularity.
    $pool->acquireTimeout(150);
} catch (\OxPHP\Shared\OperationTimeoutException $e) {
    $caught = true;
}
$elapsed = microtime(true) - $start;

if (!$caught)        { echo "FAIL: expected OperationTimeoutException, got none\n"; exit; }
if ($elapsed < 0.12) { echo "FAIL: returned too fast ({$elapsed}s)\n"; exit; }
if ($elapsed > 0.40) { echo "FAIL: returned too slow ({$elapsed}s)\n"; exit; }

// Clean up so the pool's drop path doesn't leak noisily.
$held->release();

echo "OK\n";
