<?php
/**
 * Counter smoke test — accumulator API only (swap and CAS belong to Atomic).
 */

header('Content-Type: text/plain');

$c = new OxPHP\Shared\Counter();

if ($c->get() !== 0) { echo "FAIL: initial get()\n"; exit; }
if ($c->inc() !== 1) { echo "FAIL: inc()\n"; exit; }
if ($c->inc(9) !== 10) { echo "FAIL: inc(9)\n"; exit; }
if ($c->dec() !== 9) { echo "FAIL: dec()\n"; exit; }
if ($c->add(-4) !== 5) { echo "FAIL: add(-4)\n"; exit; }
if ($c->addBatch([1, 1, 1, -3, 5]) !== 10) { echo "FAIL: addBatch\n"; exit; }
if ($c->reset() !== 10) { echo "FAIL: reset() returns prev=10\n"; exit; }
if ($c->get() !== 0) { echo "FAIL: get after reset()\n"; exit; }
if (!is_int($c->id()) || $c->id() < 1) { echo "FAIL: id\n"; exit; }

// `clone $c` invokes the registered `__clone` magic handler, which throws.
$threw = false;
try {
    $bad = clone $c;
} catch (\OxPHP\Shared\SharedException $e) {
    $threw = true;
}
if (!$threw) { echo "FAIL: clone must throw Shared\\SharedException\n"; exit; }

echo "OK\n";
