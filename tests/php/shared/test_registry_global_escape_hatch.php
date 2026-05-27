<?php
/**
 * global() returns whatever is bound under the key regardless of what
 * the factory says it would build. It is the untyped escape hatch.
 */

use OxPHP\Shared\Registry;
use OxPHP\Shared\Map;
use OxPHP\Shared\Counter;

header('Content-Type: text/plain');

// Random key so suite re-runs against the same process see a fresh
// slot every time (a Bound Map from a prior run would still pass this
// test, but the factory would not run — masking real regressions).
$key = 'test_registry_global_' . bin2hex(random_bytes(4));

$m = Registry::map($key, fn() => new Map());

// Different factory type — must NOT throw, must return the bound Map.
$got = Registry::global($key, fn() => new Counter());

if (!($got instanceof Map))      { echo "FAIL: global returned " . get_class($got) . ", expected Map\n"; exit; }
if ($got->id() !== $m->id())     { echo "FAIL: global returned different id\n"; exit; }

Registry::remove($key);

echo "OK\n";
