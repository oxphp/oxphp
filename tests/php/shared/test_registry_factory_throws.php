<?php
/**
 * When the factory throws, the Creating slot aborts (reset semantics)
 * and the exception propagates to the creator. A subsequent good
 * factory under the same key must succeed.
 */

use OxPHP\Shared\Registry;
use OxPHP\Shared\Map;

header('Content-Type: text/plain');

$key = 'test_registry_factory_throws_' . bin2hex(random_bytes(4));

$threw = false;
try {
    Registry::map($key, function () {
        throw new RuntimeException('boom');
    });
} catch (RuntimeException $e) {
    if ($e->getMessage() !== 'boom') { echo "FAIL: lost exception message: {$e->getMessage()}\n"; exit; }
    $threw = true;
}
if (!$threw) { echo "FAIL: factory throw must propagate to creator\n"; exit; }

// Slot was aborted → next call gets a fresh creator → succeeds.
$m = Registry::map($key, fn() => new Map());
if (!($m instanceof Map))            { echo "FAIL: post-abort call must succeed\n"; exit; }
if (!in_array($key, Registry::keys(), true)) { echo "FAIL: key should be bound after recovery\n"; exit; }

Registry::remove($key);

echo "OK\n";
