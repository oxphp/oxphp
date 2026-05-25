<?php
/**
 * Map — nested Shareable (Counter) lifecycle.
 */

header('Content-Type: text/plain');

$m = new OxPHP\Shared\Map();
$counter = new OxPHP\Shared\Counter(10);

$m->set('hits', $counter);

// Get the counter back; should be the same Shareable (same id).
$retrieved = $m->get('hits');
if (!($retrieved instanceof OxPHP\Shared\Counter)) {
    echo "FAIL: get returns wrong type\n";
    exit;
}
if ($retrieved->id() !== $counter->id()) {
    echo "FAIL: Shared\\Counter identity lost on get\n";
    exit;
}
if ($retrieved->get() !== 10) {
    echo "FAIL: Counter value not preserved\n";
    exit;
}

// Mutations via original or retrieved are visible through both.
$counter->add();
if ($retrieved->get() !== 11) { echo "FAIL: mutation via original not visible\n"; exit; }
$retrieved->add();
if ($counter->get() !== 12) { echo "FAIL: mutation via retrieved not visible\n"; exit; }

// pop unlinks the Map's hold and returns the value; the Counter lives on
// via the PHP var.
$removed = $m->pop('hits');
if (!($removed instanceof OxPHP\Shared\Counter)) { echo "FAIL: pop wrong type\n"; exit; }
if ($removed->id() !== $counter->id()) { echo "FAIL: pop identity\n"; exit; }
$counter->add();
if ($counter->get() !== 13) { echo "FAIL: original damaged after remove\n"; exit; }

echo "OK\n";
