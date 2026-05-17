<?php
/**
 * Mutex — int $ms timeout contract on withLockTimeout():
 *   `int $ms > 0`. Zero, negative, and non-int input throw TypeException.
 *
 * Forever / try semantics moved to dedicated methods (withLock /
 * tryWithLock) and are exercised by their own tests.
 */
header('Content-Type: text/plain');

$m = new OxPHP\Shared\Mutex(0);

// withLock (no timeout) — bare withLock($fn) succeeds and returns body's value.
$got = $m->withLock(fn(&$s) => 42);
if ($got !== 42) { echo "FAIL: withLock(\$fn) must return body value, got " . var_export($got, true) . "\n"; exit; }

// tryWithLock uncontended — succeeds.
$got = $m->tryWithLock(fn(&$s) => 'try-ok');
if ($got !== 'try-ok') { echo "FAIL: tryWithLock uncontended must succeed, got " . var_export($got, true) . "\n"; exit; }

// withLockTimeout positive — uncontended acquisition succeeds within budget.
$got = $m->withLockTimeout(fn(&$s) => 'bounded-ok', 1000);
if ($got !== 'bounded-ok') { echo "FAIL: withLockTimeout(\$fn, 1000) uncontended must succeed, got " . var_export($got, true) . "\n"; exit; }

// ms = 0 → TypeException.
$caught = null;
try {
    $m->withLockTimeout(fn(&$s) => 1, 0);
} catch (OxPHP\Shared\TypeException $e) {
    $caught = $e;
}
if ($caught === null) { echo "FAIL: withLockTimeout(\$fn, 0) must throw TypeException\n"; exit; }

// ms < 0 → TypeException.
$caught = null;
try {
    $m->withLockTimeout(fn(&$s) => 1, -1);
} catch (OxPHP\Shared\TypeException $e) {
    $caught = $e;
}
if ($caught === null) { echo "FAIL: withLockTimeout(\$fn, -1) must throw TypeException\n"; exit; }

// Non-int (float) → TypeException — the bridge enforces `int $ms` itself
// rather than relying on the engine's parameter coercion.
$caught = null;
try {
    /** @phpstan-ignore-next-line */
    $m->withLockTimeout(fn(&$s) => 1, 0.5);
} catch (OxPHP\Shared\TypeException $e) {
    $caught = $e;
}
if ($caught === null) { echo "FAIL: withLockTimeout(\$fn, 0.5) must throw TypeException\n"; exit; }

// State persistence — bare withLock mutates the stored value.
$m->withLock(function (&$s) { $s = 7; });
$snapshot = $m->withLock(fn(&$s) => $s);
if ($snapshot !== 7) { echo "FAIL: stored mutation lost, got " . var_export($snapshot, true) . "\n"; exit; }

echo "OK\n";
