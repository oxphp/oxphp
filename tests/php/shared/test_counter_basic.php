<?php
/**
 * Counter smoke test — get / set / add / compareAndSet accumulator API.
 */

header('Content-Type: text/plain');

$c = new OxPHP\Shared\Counter();

if ($c->get() !== 0)        { echo "FAIL: initial get()\n"; exit; }
if ($c->add() !== 1)        { echo "FAIL: add() default +1\n"; exit; }
if ($c->add(9) !== 10)      { echo "FAIL: add(9)\n"; exit; }
if ($c->add(-1) !== 9)      { echo "FAIL: add(-1) decrement\n"; exit; }
if ($c->add(-4) !== 5)      { echo "FAIL: add(-4)\n"; exit; }
if ($c->set(100) !== 5)     { echo "FAIL: set() returns previous=5\n"; exit; }
if ($c->get() !== 100)      { echo "FAIL: get after set()\n"; exit; }
if ($c->set(0) !== 100)     { echo "FAIL: set(0) returns previous=100 (window reset)\n"; exit; }
if ($c->get() !== 0)        { echo "FAIL: get after set(0)\n"; exit; }

// CAS: succeeds on match, no-op on mismatch.
if (!$c->compareAndSet(0, 42)) { echo "FAIL: compareAndSet(0,42) should succeed\n"; exit; }
if ($c->get() !== 42)          { echo "FAIL: get after CAS\n"; exit; }
if ($c->compareAndSet(0, 99))  { echo "FAIL: compareAndSet(0,99) should fail (current=42)\n"; exit; }
if ($c->get() !== 42)          { echo "FAIL: value changed after failed CAS\n"; exit; }

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
