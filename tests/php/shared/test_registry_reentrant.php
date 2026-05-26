<?php
/**
 * Calling Registry::<type>($key, ...) for the SAME key from inside its
 * own factory on the same thread must throw DeadlockException — not
 * self-deadlock on the per-key creation gate.
 */

use OxPHP\Shared\Registry;
use OxPHP\Shared\Map;
use OxPHP\Shared\DeadlockException;

header('Content-Type: text/plain');

$key = 'test_registry_reentrant_' . bin2hex(random_bytes(4));

$threw = false;
try {
    Registry::map($key, function () use ($key) {
        // Same key, same thread, inside the factory — would deadlock.
        return Registry::map($key, fn() => new Map());
    });
} catch (DeadlockException $e) {
    $threw = true;
}

if (!$threw) { echo "FAIL: reentrant same-key acquire must throw DeadlockException\n"; exit; }

// After the failure, the slot must be cleared (abort path) — a fresh
// non-reentrant call should now succeed.
$m = Registry::map($key, fn() => new Map());
if (!($m instanceof Map)) { echo "FAIL: post-reentrancy recovery must succeed\n"; exit; }

Registry::remove($key);

echo "OK\n";
