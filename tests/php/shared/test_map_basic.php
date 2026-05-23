<?php
/**
 * Map smoke test — construct / set / get / remove / count / clear /
 * swap / array roundtrip / id / maxEntries on the redesigned surface.
 */

header('Content-Type: text/plain');

$m = new OxPHP\Shared\Map();

// Empty state. null is the absence sentinel.
if ($m->count() !== 0) { echo "FAIL: empty count\n"; exit; }
if ($m->get('missing') !== null) { echo "FAIL: empty get\n"; exit; }
if ($m->maxEntries() !== null) { echo "FAIL: unbounded maxEntries\n"; exit; }
if ($m->id() <= 0) { echo "FAIL: id must be positive\n"; exit; }

// Scalar writes (null is NOT a storable value — covered separately).
$m->set('alpha', 1);
$m->set('beta', 'hello');
$m->set('gamma', 3.14);
$m->set('eps', true);

if ($m->count() !== 4) { echo "FAIL: count after 4 sets\n"; exit; }
if ($m->get('alpha') !== 1) { echo "FAIL: get alpha\n"; exit; }
if ($m->get('beta') !== 'hello') { echo "FAIL: get beta\n"; exit; }
if ($m->get('gamma') !== 3.14) { echo "FAIL: get gamma\n"; exit; }
if ($m->get('eps') !== true) { echo "FAIL: get eps\n"; exit; }

// Overwrite via set (no return) and via swap (returns prev).
$m->set('alpha', 100);
if ($m->get('alpha') !== 100) { echo "FAIL: overwrite\n"; exit; }
if ($m->swap('alpha', 200) !== 100) { echo "FAIL: swap returns prev\n"; exit; }
if ($m->get('alpha') !== 200) { echo "FAIL: swap stored\n"; exit; }
if ($m->count() !== 4) { echo "FAIL: count after overwrite\n"; exit; }

// remove returns whether the key existed.
if ($m->remove('alpha') !== true) { echo "FAIL: remove existing → true\n"; exit; }
if ($m->remove('alpha') !== false) { echo "FAIL: remove missing → false\n"; exit; }
if ($m->count() !== 3) { echo "FAIL: count after remove\n"; exit; }

// Array value roundtrip (scalar + assoc).
$m->set('list', ['x', 'y', 'z']);
if ($m->get('list') !== ['x', 'y', 'z']) { echo "FAIL: array roundtrip\n"; exit; }
$m->set('conf', ['timeout' => 5, 'retries' => 3]);
if ($m->get('conf') !== ['timeout' => 5, 'retries' => 3]) { echo "FAIL: assoc roundtrip\n"; exit; }

// clear returns the removed count.
$removed = $m->clear();
if ($removed !== 5) { echo "FAIL: clear returned $removed (want 5)\n"; exit; }
if ($m->count() !== 0) { echo "FAIL: count after clear\n"; exit; }

echo "OK\n";
