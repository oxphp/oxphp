<?php
/**
 * Map — per-instance cap enforcement via CapacityException.
 */

header('Content-Type: text/plain');

$m = new OxPHP\Shared\Map(3);

if ($m->maxEntries() !== 3) { echo "FAIL: maxEntries getter\n"; exit; }

$m->set('k1', 1);
$m->set('k2', 2);
$m->set('k3', 3);

// Fourth new key must trip.
$threw = false;
try {
    $m->set('k4', 4);
} catch (OxPHP\Shared\CapacityException $e) {
    $threw = true;
}
if (!$threw) { echo "FAIL: cap must throw CapacityException\n"; exit; }
if ($m->count() !== 3) { echo "FAIL: count unchanged on reject\n"; exit; }

// Overwrite existing key is allowed at cap.
$m->set('k1', 100);
if ($m->get('k1') !== 100) { echo "FAIL: overwrite at cap\n"; exit; }
if ($m->count() !== 3) { echo "FAIL: count still 3 after overwrite\n"; exit; }

// Remove frees a slot.
$m->remove('k1');
if ($m->count() !== 2) { echo "FAIL: count after remove\n"; exit; }
$m->set('k4', 4);  // now fits
if ($m->count() !== 3) { echo "FAIL: count after refill\n"; exit; }

// trySet also respects cap.
$threw = false;
try {
    $m->trySet('k5', 5);
} catch (OxPHP\Shared\CapacityException $e) {
    $threw = true;
}
if (!$threw) { echo "FAIL: trySet must respect cap\n"; exit; }

// Invalid maxEntries → TypeException.
$threw = false;
try {
    new OxPHP\Shared\Map(0);
} catch (OxPHP\Shared\TypeException $e) {
    $threw = true;
}
if (!$threw) { echo "FAIL: zero maxEntries must throw\n"; exit; }

$threw = false;
try {
    new OxPHP\Shared\Map(-5);
} catch (OxPHP\Shared\TypeException $e) {
    $threw = true;
}
if (!$threw) { echo "FAIL: negative maxEntries must throw\n"; exit; }

echo "OK\n";
