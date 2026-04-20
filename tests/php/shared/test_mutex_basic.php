<?php
header('Content-Type: text/plain');

$m = new OxPHP\Shared\Mutex(0);

$v = $m->with(fn(&$s) => $s);
if ($v !== 0) { echo "FAIL: initial read\n"; exit; }

$new = $m->with(function(&$s) {
    $s = 42;
    return $s;
});
if ($new !== 42) { echo "FAIL: set returns new value\n"; exit; }

if ($m->with(fn(&$s) => $s) !== 42) { echo "FAIL: mutation persisted\n"; exit; }

$t = $m->tryWith(fn(&$s) => $s + 1);
if ($t !== 43) { echo "FAIL: tryWith uncontended\n"; exit; }

if ($m->isPoisoned()) { echo "FAIL: initially not poisoned\n"; exit; }
$m->clearPoison();
if ($m->isPoisoned()) { echo "FAIL: clearPoison leaves clear\n"; exit; }

if (!is_int($m->id())) { echo "FAIL: id() type\n"; exit; }

echo "OK\n";
