<?php
/**
 * Pool smoke test — create + acquire + release + size/stats.
 *
 * Uses stdClass as the pool resource (simplest v1-compliant object).
 * The factory is called once on first acquire; subsequent acquires
 * reuse the same slot. release() zeroes the Handle so a double-
 * release is rejected at the Handle layer.
 */

header('Content-Type: text/plain');

$minted = 0;
// Positional: (factory, destroy, maxSize, idleTimeout, defaultAcquireTimeout).
$pool = new OxPHP\Shared\Pool(
    function () use (&$minted): object {
        $minted++;
        $r = new stdClass();
        $r->tag = 'resource-' . $minted;
        return $r;
    },
    null,  // destroy
    2,     // maxSize
);

if ($pool->maxSize() !== 2) { echo "FAIL: maxSize\n"; exit; }
if ($pool->size() !== 0)    { echo "FAIL: initial size\n"; exit; }
if ($pool->inUse() !== 0)   { echo "FAIL: initial inUse\n"; exit; }
if ($pool->idle() !== 0)    { echo "FAIL: initial idle\n"; exit; }
if (!is_int($pool->id()) || $pool->id() < 1) { echo "FAIL: id\n"; exit; }

$h = $pool->acquire();
if (!($h instanceof OxPHP\Shared\Pool\Handle)) { echo "FAIL: acquire returns Handle\n"; exit; }
if ($minted !== 1)       { echo "FAIL: factory should run on first acquire\n"; exit; }
if ($pool->size() !== 1) { echo "FAIL: size after acquire\n"; exit; }
if ($pool->inUse() !== 1){ echo "FAIL: inUse after acquire\n"; exit; }
if ($pool->idle() !== 0) { echo "FAIL: idle after acquire\n"; exit; }

$r = $h->get();
if (!($r instanceof stdClass)) { echo "FAIL: Handle::get returns resource\n"; exit; }
if ($r->tag !== 'resource-1')  { echo "FAIL: resource identity\n"; exit; }

$pool->release($h);
if ($pool->inUse() !== 0) { echo "FAIL: inUse after release\n"; exit; }
if ($pool->idle() !== 1)  { echo "FAIL: idle after release\n"; exit; }
if ($pool->size() !== 1)  { echo "FAIL: size stable after release\n"; exit; }

// Second acquire reuses the pooled resource — factory count unchanged.
$h2 = $pool->acquire();
if ($minted !== 1)          { echo "FAIL: factory should NOT re-run\n"; exit; }
$r2 = $h2->get();
if ($r2->tag !== 'resource-1') { echo "FAIL: same resource on second acquire\n"; exit; }

$pool->release($h2);

// Double-release via stale handle must fail with TypeException.
$threw = false;
try {
    $pool->release($h2);
} catch (\OxPHP\Shared\TypeException $e) {
    $threw = true;
}
if (!$threw) { echo "FAIL: double-release must throw TypeException\n"; exit; }

echo "OK\n";
