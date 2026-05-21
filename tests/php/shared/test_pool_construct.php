<?php
/**
 * Pool::__construct() argument validation: maxSize must be > 0, and
 * idleTimeoutMs must be >= 0 (0 disables idle eviction).
 */
header('Content-Type: text/plain');

foreach ([0, -1] as $badMax) {
    $threw = false;
    try {
        new OxPHP\Shared\Pool(fn(): object => new stdClass(), null, $badMax);
    } catch (OxPHP\Shared\TypeException $e) {
        $threw = true;
    }
    if (!$threw) { echo "FAIL: maxSize=$badMax must throw TypeException\n"; exit; }
}

$threw = false;
try {
    new OxPHP\Shared\Pool(fn(): object => new stdClass(), null, 4, -1);
} catch (OxPHP\Shared\TypeException $e) {
    $threw = true;
}
if (!$threw) { echo "FAIL: idleTimeoutMs=-1 must throw TypeException\n"; exit; }

// idleTimeoutMs = 0 is valid (disables idle eviction) — must construct.
$pool = new OxPHP\Shared\Pool(fn(): object => new stdClass(), null, 4, 0);
if (!$pool instanceof OxPHP\Shared\Pool) { echo "FAIL: idleTimeoutMs=0 must be accepted\n"; exit; }
$h = $pool->acquire();
$h->release();

echo "OK\n";
