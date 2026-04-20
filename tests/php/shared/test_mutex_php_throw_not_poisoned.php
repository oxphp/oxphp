<?php
header('Content-Type: text/plain');

$m = new OxPHP\Shared\Mutex(0);

try {
    $m->with(function(&$s) {
        $s = 99;
        throw new \RuntimeException();
    });
} catch (\RuntimeException $e) {}

if ($m->isPoisoned()) { echo "FAIL: PHP throw should NOT poison (default)\n"; exit; }
if ($m->with(fn(&$s) => $s) !== 99) { echo "FAIL: partial mutation lost\n"; exit; }

echo "OK\n";
