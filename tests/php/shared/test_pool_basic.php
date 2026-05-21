<?php
/**
 * Pool smoke test — create + acquire + get + release + reuse, plus the
 * stats() snapshot.
 *
 * Uses stdClass as the pool resource (simplest v1-compliant object).
 * The factory is called once on first acquire; subsequent acquires
 * reuse the same slot. Release edge cases (idempotency, get-after-
 * release) live in test_pool_handle_release.
 */

header('Content-Type: text/plain');

$minted = 0;
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

$s = $pool->stats();
if ($s->maxSize() !== 2) { echo "FAIL: maxSize\n"; exit; }
if ($s->size() !== 0)    { echo "FAIL: initial size\n"; exit; }
if ($s->inUse() !== 0)   { echo "FAIL: initial inUse\n"; exit; }
if ($s->idle() !== 0)    { echo "FAIL: initial idle\n"; exit; }
if (!is_int($pool->id()) || $pool->id() < 1) { echo "FAIL: id\n"; exit; }

$h = $pool->acquire();
if (!($h instanceof OxPHP\Shared\Pool\Handle)) { echo "FAIL: acquire returns Handle\n"; exit; }
if ($minted !== 1)       { echo "FAIL: factory should run on first acquire\n"; exit; }
$s = $pool->stats();
if ($s->size() !== 1)  { echo "FAIL: size after acquire\n"; exit; }
if ($s->inUse() !== 1) { echo "FAIL: inUse after acquire\n"; exit; }
if ($s->idle() !== 0)  { echo "FAIL: idle after acquire\n"; exit; }

$r = $h->get();
if (!($r instanceof stdClass)) { echo "FAIL: Handle::get returns resource\n"; exit; }
if ($r->tag !== 'resource-1')  { echo "FAIL: resource identity\n"; exit; }

$h->release();
$s = $pool->stats();
if ($s->inUse() !== 0) { echo "FAIL: inUse after release\n"; exit; }
if ($s->idle() !== 1)  { echo "FAIL: idle after release\n"; exit; }
if ($s->size() !== 1)  { echo "FAIL: size stable after release\n"; exit; }

// Second acquire reuses the pooled resource — factory count unchanged.
$h2 = $pool->acquire();
if ($minted !== 1)          { echo "FAIL: factory should NOT re-run\n"; exit; }
$r2 = $h2->get();
if ($r2->tag !== 'resource-1') { echo "FAIL: same resource on second acquire\n"; exit; }
$h2->release();

echo "OK\n";
