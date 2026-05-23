<?php
// String keys are binary-safe (opaque bytes) — like PHP arrays / Go / Redis.
// Non-UTF-8 keys (incl. embedded NUL) round-trip faithfully and stay distinct
// across the single-key and batch paths.
header('Content-Type: text/plain');
$m = new OxPHP\Shared\Map();

$k1 = "\xff\xfe";
$k2 = "\xff\x00bar";   // embedded NUL
$k3 = "\xfe";

// Single-key paths.
$m->set($k1, 1);
$m->set($k2, 2);
$m->set($k3, 3);
if ($m->get($k1) !== 1 || $m->get($k2) !== 2 || $m->get($k3) !== 3) {
    echo "FAIL: binary keys not distinct on get\n"; exit;
}
if ($m->count() !== 3) {
    echo "FAIL: count != 3 (binary keys collapsed?) got " . $m->count() . "\n"; exit;
}
if ($m->swap($k1, 11) !== 1) { echo "FAIL: swap binary\n"; exit; }
if ($m->pop($k1) !== 11) { echo "FAIL: pop binary\n"; exit; }
if (!$m->compareAndSet($k2, 2, 22)) { echo "FAIL: compareAndSet binary\n"; exit; }
if ($m->get($k2) !== 22) { echo "FAIL: CAS binary value\n"; exit; }

// int 255 and "\xff" are distinct keys.
$m->set(255, 'int');
$m->set("\xff", 'bytes');
if ($m->get(255) !== 'int' || $m->get("\xff") !== 'bytes') {
    echo "FAIL: int 255 vs \"\\xff\" collided\n"; exit;
}

// Batch: setMany with binary ARRAY keys — distinct, not collapsed to "".
$b = new OxPHP\Shared\Map();
$n = $b->setMany(["\xff\xfe" => 1, "\xff\x00" => 2]);
if ($n !== 2) { echo "FAIL: setMany binary keys collapsed, n=$n\n"; exit; }
if ($b->count() !== 2) { echo "FAIL: setMany count != 2\n"; exit; }
if ($b->get("\xff\xfe") !== 1 || $b->get("\xff\x00") !== 2) {
    echo "FAIL: setMany binary values wrong\n"; exit;
}

// getMany / removeMany over binary keys.
$got = [];
foreach ($b->getMany(["\xff\xfe", "\xff\x00"]) as $k => $v) {
    $got[bin2hex($k)] = $v;
}
$want = [bin2hex("\xff\xfe") => 1, bin2hex("\xff\x00") => 2];
if ($got !== $want) { echo "FAIL: getMany binary: " . json_encode($got) . "\n"; exit; }
if ($b->removeMany(["\xff\xfe"]) !== 1) { echo "FAIL: removeMany binary\n"; exit; }
if ($b->count() !== 1) { echo "FAIL: removeMany count\n"; exit; }

// forEach surfaces binary keys back to PHP.
$seen = [];
$b->forEach(function ($k, $v) use (&$seen) { $seen[bin2hex($k)] = $v; });
if ($seen !== [bin2hex("\xff\x00") => 2]) {
    echo "FAIL: forEach binary key: " . json_encode($seen) . "\n"; exit;
}

echo "OK\n";
