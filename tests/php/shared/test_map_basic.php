<?php
/**
 * Map smoke test — construct / set / get / has / remove /
 * count / clear / keys / setIfAbsent / id / maxEntries.
 */

header('Content-Type: text/plain');

$m = new OxPHP\Shared\Map();

// Empty state.
if ($m->count() !== 0) { echo "FAIL: empty count\n"; exit; }
if ($m->has('any')) { echo "FAIL: empty has\n"; exit; }
if ($m->get('missing') !== null) { echo "FAIL: empty get\n"; exit; }
if ($m->get('missing', 'fallback') !== 'fallback') { echo "FAIL: default\n"; exit; }
if ($m->maxEntries() !== null) { echo "FAIL: unbounded maxEntries\n"; exit; }
if ($m->id() <= 0) { echo "FAIL: id must be positive\n"; exit; }

// Scalar writes.
$m->set('alpha', 1);
$m->set('beta', 'hello');
$m->set('gamma', 3.14);
$m->set('delta', null);
$m->set('eps', true);

if ($m->count() !== 5) { echo "FAIL: count after 5 sets\n"; exit; }
if ($m->get('alpha') !== 1) { echo "FAIL: get alpha\n"; exit; }
if ($m->get('beta') !== 'hello') { echo "FAIL: get beta\n"; exit; }
if ($m->get('gamma') !== 3.14) { echo "FAIL: get gamma\n"; exit; }
if ($m->get('delta') !== null) { echo "FAIL: get delta\n"; exit; }
if ($m->get('eps') !== true) { echo "FAIL: get eps\n"; exit; }

// Overwrite.
$m->set('alpha', 100);
if ($m->get('alpha') !== 100) { echo "FAIL: overwrite\n"; exit; }
if ($m->count() !== 5) { echo "FAIL: count after overwrite\n"; exit; }

// has.
if (!$m->has('alpha')) { echo "FAIL: has existing\n"; exit; }
if ($m->has('missing')) { echo "FAIL: has missing\n"; exit; }

// remove.
$prev = $m->remove('alpha');
if ($prev !== 100) { echo "FAIL: remove returns prev\n"; exit; }
if ($m->has('alpha')) { echo "FAIL: still has after remove\n"; exit; }
if ($m->count() !== 4) { echo "FAIL: count after remove\n"; exit; }

// remove missing → null.
if ($m->remove('nope') !== null) { echo "FAIL: remove missing\n"; exit; }

// setIfAbsent.
$new = $m->setIfAbsent('fresh', 42);
if ($new !== true) { echo "FAIL: setIfAbsent new\n"; exit; }
$again = $m->setIfAbsent('fresh', 999);
if ($again !== false) { echo "FAIL: setIfAbsent existing\n"; exit; }
if ($m->get('fresh') !== 42) { echo "FAIL: value preserved by setIfAbsent\n"; exit; }

// Array value.
$m->set('list', ['x', 'y', 'z']);
$got = $m->get('list');
if ($got !== ['x', 'y', 'z']) { echo "FAIL: array roundtrip\n"; exit; }

// Associative array value.
$m->set('conf', ['timeout' => 5, 'retries' => 3]);
$conf = $m->get('conf');
if ($conf !== ['timeout' => 5, 'retries' => 3]) { echo "FAIL: assoc roundtrip\n"; exit; }

// keys snapshot.
$keys = $m->keys();
sort($keys);
$expected = ['beta', 'conf', 'delta', 'eps', 'fresh', 'gamma', 'list'];
if ($keys !== $expected) {
    echo "FAIL: keys expected " . json_encode($expected) . " got " . json_encode($keys) . "\n";
    exit;
}

// clear.
$m->clear();
if ($m->count() !== 0) { echo "FAIL: count after clear\n"; exit; }
if ($m->has('beta')) { echo "FAIL: has after clear\n"; exit; }

echo "OK\n";
