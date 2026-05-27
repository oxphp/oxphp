<?php
/**
 * keys() lists currently-bound key names. remove() drops one out.
 */

use OxPHP\Shared\Registry;
use OxPHP\Shared\Counter;
use OxPHP\Shared\Map;

header('Content-Type: text/plain');

// Pre-condition: this test owns its names; assert presence, not exact set
// (other in-flight requests on the same process may have their own keys).
$key_a = 'test_registry_keys_a_' . bin2hex(random_bytes(4));
$key_b = 'test_registry_keys_b_' . bin2hex(random_bytes(4));

Registry::counter($key_a, fn() => new Counter());
Registry::map($key_b, fn() => new Map());

$keys = Registry::keys();
if (!is_array($keys))           { echo "FAIL: keys() must return array\n"; exit; }
if (!in_array($key_a, $keys, true)) { echo "FAIL: keys() missing $key_a\n"; exit; }
if (!in_array($key_b, $keys, true)) { echo "FAIL: keys() missing $key_b\n"; exit; }

Registry::remove($key_a);

$keys2 = Registry::keys();
if (in_array($key_a, $keys2, true))  { echo "FAIL: removed key still in keys()\n"; exit; }
if (!in_array($key_b, $keys2, true)) { echo "FAIL: untouched key disappeared\n"; exit; }

Registry::remove($key_b);

echo "OK\n";
