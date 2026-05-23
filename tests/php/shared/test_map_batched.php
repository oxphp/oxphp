<?php
/**
 * Map — setMany / getMany / removeMany on the redesigned surface.
 * getMany returns a lazy iterator that SKIPS absent keys.
 */

header('Content-Type: text/plain');

$m = new OxPHP\Shared\Map();

// setMany returns inserted count (accepts an assoc array — it is iterable).
$inserted = $m->setMany([
    'a' => 1,
    'b' => 2,
    'c' => 3,
    'd' => 4,
]);
if ($inserted !== 4) { echo "FAIL: setMany count got $inserted\n"; exit; }
if ($m->count() !== 4) { echo "FAIL: count after setMany\n"; exit; }

// getMany yields present keys only; absent keys are skipped (not null).
$got = [];
foreach ($m->getMany(['a', 'b', 'missing', 'd']) as $k => $v) {
    $got[$k] = $v;
}
$expected = ['a' => 1, 'b' => 2, 'd' => 4];
if ($got !== $expected) {
    echo "FAIL: getMany\nexpected: " . json_encode($expected) . "\ngot: " . json_encode($got) . "\n";
    exit;
}

// Empty getMany.
$got = [];
foreach ($m->getMany([]) as $k => $v) { $got[$k] = $v; }
if ($got !== []) { echo "FAIL: empty getMany\n"; exit; }

// removeMany counts actual deletions.
$removed = $m->removeMany(['a', 'c', 'notthere']);
if ($removed !== 2) { echo "FAIL: removeMany count got $removed\n"; exit; }
if ($m->count() !== 2) { echo "FAIL: count after removeMany\n"; exit; }
if ($m->get('a') !== null || $m->get('c') !== null) { echo "FAIL: removed keys still present\n"; exit; }
if ($m->get('b') !== 2 || $m->get('d') !== 4) { echo "FAIL: surviving keys missing\n"; exit; }

// setMany with cap — partial success, bails at the over-cap key.
$capped = new OxPHP\Shared\Map(2);
$threw = false;
try {
    $capped->setMany(['x' => 1, 'y' => 2, 'z' => 3]);
} catch (OxPHP\Shared\CapacityException $e) {
    $threw = true;
}
if (!$threw) { echo "FAIL: setMany must throw on cap breach\n"; exit; }
if ($capped->count() !== 2) { echo "FAIL: partial inserts lost\n"; exit; }

echo "OK\n";
