<?php
/**
 * Pool::stats() — returns an immutable Pool\Stats snapshot with a
 * consistent size invariant (size() == inUse() + idle()) and a
 * utilization() helper. Counters are read via accessor methods.
 */
header('Content-Type: text/plain');

$pool = new OxPHP\Shared\Pool(fn(): object => new stdClass(), null, 4);

$s = $pool->stats();
if (!$s instanceof OxPHP\Shared\Pool\Stats) { echo "FAIL: stats() must return Pool\\Stats\n"; exit; }
if ($s->maxSize() !== 4) { echo "FAIL: maxSize, got {$s->maxSize()}\n"; exit; }
if ($s->size() !== $s->inUse() + $s->idle()) { echo "FAIL: size invariant on empty pool\n"; exit; }
if (abs($s->utilization() - 0.0) > 1e-9) { echo "FAIL: utilization on empty pool must be 0.0\n"; exit; }

$h1 = $pool->acquire();
$h2 = $pool->acquire();
$s = $pool->stats();
if ($s->inUse() !== 2) { echo "FAIL: inUse after 2 acquires, got {$s->inUse()}\n"; exit; }
if ($s->size() !== $s->inUse() + $s->idle()) { echo "FAIL: size invariant under load\n"; exit; }
if (abs($s->utilization() - 0.5) > 1e-9) {
    echo "FAIL: utilization 2/4 must be 0.5, got " . $s->utilization() . "\n"; exit;
}

// A Stats instance is a point-in-time snapshot, not a live view: mutating
// the pool afterwards must not change counters already captured.
$before = $pool->stats();
$h1->release();
if ($before->inUse() !== 2) { echo "FAIL: snapshot must not track later state, got {$before->inUse()}\n"; exit; }

$s = $pool->stats();
if ($s->inUse() !== 1) { echo "FAIL: inUse after release, got {$s->inUse()}\n"; exit; }

$h2->release();

echo "OK\n";
