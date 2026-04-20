<?php
/**
 * Default acquire timeout from constructor.
 *
 * Complements test_pool_timeout.php (which passes an explicit
 * `acquire($timeout)`): here we verify that the 5th constructor
 * argument (`defaultAcquireTimeout`) is honoured when `acquire()`
 * is called without a parameter.
 *
 * Holds the only slot, then calls parameter-less acquire() —
 * must throw TimeoutException after ~defaultAcquireTimeout
 * seconds, regardless of what the pool-wide default would be.
 */

header('Content-Type: text/plain');

$pool = new OxPHP\Shared\Pool(
    fn(): object => new stdClass(),
    null,   // destroy
    1,      // maxSize
    300.0,  // idleTimeout — unused here
    0.15,   // defaultAcquireTimeout
);

$held = $pool->acquire();
if ($pool->inUse() !== 1) { echo "FAIL: inUse=1 expected\n"; exit; }

$start = microtime(true);
$caught = false;
try {
    $pool->acquire(); // no param → must use defaultAcquireTimeout
} catch (\OxPHP\Shared\TimeoutException $e) {
    $caught = true;
}
$elapsed = microtime(true) - $start;

if (!$caught)        { echo "FAIL: expected TimeoutException\n"; exit; }
if ($elapsed < 0.12) { echo "FAIL: returned too fast ({$elapsed}s) — default not honoured?\n"; exit; }
if ($elapsed > 0.40) { echo "FAIL: returned too slow ({$elapsed}s)\n"; exit; }

$pool->release($held);

echo "OK\n";
