<?php
/**
 * Mutex — a PHP throw inside withLock() leaves the mutex usable.
 *
 * The previous `isPoisoned()` accessor is gone; the equivalent assertion
 * is "withLock still succeeds after the throw" — and the partial mutation
 * persists, matching the documented behavior.
 */
header('Content-Type: text/plain');

$m = new OxPHP\Shared\Mutex(0);

try {
    $m->withLock(function(&$s) {
        $s = 99;
        throw new \RuntimeException();
    });
} catch (\RuntimeException $e) {}

// Subsequent withLock must succeed and observe the partial mutation.
if ($m->withLock(fn(&$s) => $s) !== 99) { echo "FAIL: partial mutation lost\n"; exit; }

echo "OK\n";
