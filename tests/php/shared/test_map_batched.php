<?php
/**
 * Map — setMany / getMany / removeMany.
 */

header('Content-Type: text/plain');

$m = new OxPHP\Shared\Map();

// setMany returns inserted count.
$inserted = $m->setMany([
    'a' => 1,
    'b' => 2,
    'c' => 3,
    'd' => 4,
]);
if ($inserted !== 4) { echo "FAIL: setMany count got $inserted\n"; exit; }
if ($m->count() !== 4) { echo "FAIL: count after setMany\n"; exit; }

// getMany returns keyed array; missing keys map to null.
$got = $m->getMany(['a', 'b', 'missing', 'd']);
$expected = ['a' => 1, 'b' => 2, 'missing' => null, 'd' => 4];
if ($got !== $expected) {
    echo "FAIL: getMany\nexpected: " . json_encode($expected) . "\ngot: " . json_encode($got) . "\n";
    exit;
}

// Empty getMany.
$got = $m->getMany([]);
if ($got !== []) { echo "FAIL: empty getMany\n"; exit; }

// removeMany counts actual deletions.
$removed = $m->removeMany(['a', 'c', 'notthere']);
if ($removed !== 2) { echo "FAIL: removeMany count got $removed\n"; exit; }
if ($m->count() !== 2) { echo "FAIL: count after removeMany\n"; exit; }
if ($m->has('a') || $m->has('c')) { echo "FAIL: removed keys still present\n"; exit; }
if (!$m->has('b') || !$m->has('d')) { echo "FAIL: surviving keys missing\n"; exit; }

// setMany with cap — partial success.
$capped = new OxPHP\Shared\Map(2);
$threw = false;
try {
    $capped->setMany(['x' => 1, 'y' => 2, 'z' => 3]);
} catch (OxPHP\Shared\CapacityException $e) {
    $threw = true;
}
if (!$threw) { echo "FAIL: setMany must throw on cap breach\n"; exit; }
// First two should have landed (per-key atomic, bail at 3rd).
if ($capped->count() !== 2) { echo "FAIL: partial inserts lost\n"; exit; }

echo "OK\n";
