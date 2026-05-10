<?php
/**
 * Atomic smoke test — all 9 methods with default Ordering::SeqCst.
 */

header('Content-Type: text/plain');

$a = new OxPHP\Shared\Atomic(initial: 100);

if ($a->load() !== 100) { echo "FAIL: load() initial\n"; exit; }

$a->store(7);
if ($a->load() !== 7) { echo "FAIL: store/load\n"; exit; }

if ($a->swap(42) !== 7) { echo "FAIL: swap returns prev\n"; exit; }
if ($a->load() !== 42) { echo "FAIL: load after swap\n"; exit; }

if (!$a->compareAndSet(42, 99)) { echo "FAIL: cas hit\n"; exit; }
if ($a->compareAndSet(42, 1)) { echo "FAIL: cas miss should be false\n"; exit; }
if ($a->load() !== 99) { echo "FAIL: load after cas\n"; exit; }

if ($a->fetchAdd(1) !== 99) { echo "FAIL: fetchAdd returns prev\n"; exit; }
if ($a->load() !== 100) { echo "FAIL: load after fetchAdd\n"; exit; }

if ($a->fetchSub(50) !== 100) { echo "FAIL: fetchSub returns prev\n"; exit; }
if ($a->load() !== 50) { echo "FAIL: load after fetchSub\n"; exit; }

$a->store(0b1010);
if ($a->fetchAnd(0b1100) !== 0b1010) { echo "FAIL: fetchAnd prev\n"; exit; }
if ($a->load() !== 0b1000) { echo "FAIL: fetchAnd result\n"; exit; }
if ($a->fetchOr(0b0011) !== 0b1000) { echo "FAIL: fetchOr prev\n"; exit; }
if ($a->load() !== 0b1011) { echo "FAIL: fetchOr result\n"; exit; }
if ($a->fetchXor(0b1111) !== 0b1011) { echo "FAIL: fetchXor prev\n"; exit; }
if ($a->load() !== 0b0100) { echo "FAIL: fetchXor result\n"; exit; }

if (!is_int($a->id()) || $a->id() < 1) { echo "FAIL: id\n"; exit; }

// Cloning is forbidden, same as Counter / other Shared types.
$threw = false;
try {
    $bad = clone $a;
} catch (\OxPHP\Shared\SharedException $e) {
    $threw = true;
}
if (!$threw) { echo "FAIL: clone must throw\n"; exit; }

echo "OK\n";
