<?php
/**
 * Counter smoke test — basic arithmetic + CAS + addBatch.
 */

header('Content-Type: text/plain');

$c = new OxPHP\Shared\Counter();

if ($c->get() !== 0) { echo "FAIL: initial get()\n"; exit; }
if ($c->inc() !== 1) { echo "FAIL: inc()\n"; exit; }
if ($c->inc(9) !== 10) { echo "FAIL: inc(9)\n"; exit; }
if ($c->dec() !== 9) { echo "FAIL: dec()\n"; exit; }
if ($c->add(-4) !== 5) { echo "FAIL: add(-4)\n"; exit; }
if ($c->swap(100) !== 5) { echo "FAIL: swap() should return prev=5\n"; exit; }
if ($c->get() !== 100) { echo "FAIL: get after swap\n"; exit; }
if (!$c->compareAndSet(100, 200)) { echo "FAIL: cas hit\n"; exit; }
if ($c->compareAndSet(999, 0)) { echo "FAIL: cas miss should return false\n"; exit; }
if ($c->addBatch([1, 1, 1, -3, 5]) !== 205) { echo "FAIL: addBatch\n"; exit; }
if ($c->swap(0) !== 205) { echo "FAIL: swap(0) returns prev\n"; exit; }
if ($c->get() !== 0) { echo "FAIL: get after swap(0)\n"; exit; }
if (!is_int($c->id()) || $c->id() < 1) { echo "FAIL: id\n"; exit; }

// `clone $c` invokes the registered `__clone` magic handler, which
// throws per spec.
$threw = false;
try {
    $bad = clone $c;
} catch (\OxPHP\Shared\SharedException $e) {
    $threw = true;
}
if (!$threw) { echo "FAIL: clone must throw Shared\\SharedException\n"; exit; }

echo "OK\n";
