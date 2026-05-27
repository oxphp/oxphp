<?php
/**
 * remove() drops the name binding + pin. Existing handles keep working
 * on the now-anonymous entry; the next typed call under the same key
 * creates a NEW entry (different id). Documented namespace-management
 * semantics: captured handles do NOT auto-converge on the new entry.
 */

use OxPHP\Shared\Registry;
use OxPHP\Shared\Counter;

header('Content-Type: text/plain');

// Random key so suite re-runs against the same process don't collide
// with a leftover Bound pin (factory default '5' would not be applied
// the second time).
$key = 'test_registry_remove_' . bin2hex(random_bytes(4));

$old = Registry::counter($key, fn() => new Counter(5));
$old_id = $old->id();
if ($old->get() !== 5) { echo "FAIL: factory not applied\n"; exit; }

if (Registry::remove($key) !== true)  { echo "FAIL: remove(\$key) must return true on first call\n"; exit; }
if (Registry::remove($key) !== false) { echo "FAIL: remove(\$key) must return false the second time\n"; exit; }

// Old handle still mutates the now-anonymous old entry.
$old->add(); // 6
if ($old->get() !== 6) { echo "FAIL: old handle should still work after remove\n"; exit; }

// New typed call creates a NEW entry — different id.
$new = Registry::counter($key, fn() => new Counter(0));
if ($new->id() === $old_id) { echo "FAIL: remove+recreate must produce a different id\n"; exit; }
if ($new->get() !== 0)      { echo "FAIL: new entry should start at factory default\n"; exit; }

// Absent key.
if (Registry::remove('test_registry_remove_never_' . bin2hex(random_bytes(4))) !== false) {
    echo "FAIL: remove of absent key must return false\n"; exit;
}

Registry::remove($key);

echo "OK\n";
