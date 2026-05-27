<?php
/**
 * A key bound under one Shared type must reject acquisition under
 * another with TypeException.
 */

use OxPHP\Shared\Registry;
use OxPHP\Shared\Map;
use OxPHP\Shared\Counter;
use OxPHP\Shared\TypeException;

header('Content-Type: text/plain');

// Random key so suite re-runs against the same process don't carry a
// leftover Bound entry that would mask the first-bind verification.
$key = 'test_registry_type_mismatch_' . bin2hex(random_bytes(4));

$m = Registry::map($key, fn() => new Map());
if (!($m instanceof Map)) { echo "FAIL: map() did not return Map\n"; exit; }

$threw = false;
try {
    Registry::counter($key, fn() => new Counter());
} catch (TypeException $e) {
    $threw = true;
}
if (!$threw) { echo "FAIL: type-mismatched typed call must throw TypeException\n"; exit; }

Registry::remove($key);

echo "OK\n";
