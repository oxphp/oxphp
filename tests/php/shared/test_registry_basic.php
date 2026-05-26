<?php
/**
 * Two Registry::map() calls with the same key in one request return the
 * same registry id, and the factory runs exactly once.
 */

use OxPHP\Shared\Registry;
use OxPHP\Shared\Map;

header('Content-Type: text/plain');

// Random key so the test survives suite re-runs against the same
// long-lived OxPHP process — a fixed key would carry a Bound pin from
// the prior run and skip the factory, failing the `$runs === 1` check.
$key = 'test_registry_basic_' . bin2hex(random_bytes(4));

$runs = 0;
$a = Registry::map($key, function () use (&$runs) {
    $runs++;
    return new Map();
});
$a->set('x', 1);

$b = Registry::map($key, function () use (&$runs) {
    $runs++;
    return new Map();
});

if ($a->id() !== $b->id()) { echo "FAIL: ids differ {$a->id()} vs {$b->id()}\n"; exit; }
if ($runs !== 1)           { echo "FAIL: factory ran $runs times, expected 1\n"; exit; }
if ($b->get('x') !== 1)    { echo "FAIL: shared value not visible via second handle\n"; exit; }

Registry::remove($key);

echo "OK\n";
