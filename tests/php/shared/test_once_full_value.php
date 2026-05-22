<?php
// Once stores full SharedValue: arrays round-trip through get/getOrInit/trySet.
header('Content-Type: text/plain');

use OxPHP\Shared\Once;

$o = new Once();
$v = $o->getOrInit(fn() => [1, 2, 3]);
if ($v !== [1, 2, 3]) { echo "FAIL: array factory result\n"; exit; }
if ($o->get() !== [1, 2, 3]) { echo "FAIL: array cached read\n"; exit; }

$o2 = new Once();
if (!$o2->trySet(['a' => 1, 'b' => 2])) { echo "FAIL: trySet assoc array\n"; exit; }
$got = $o2->get();
if (($got['a'] ?? null) !== 1 || ($got['b'] ?? null) !== 2) { echo "FAIL: assoc array read\n"; exit; }

// Nested array.
$o3 = new Once();
$o3->getOrInit(fn() => ['db' => ['host' => 'localhost', 'port' => 5432]]);
$cfg = $o3->get();
if (($cfg['db']['port'] ?? null) !== 5432) { echo "FAIL: nested array read\n"; exit; }

echo "OK\n";
