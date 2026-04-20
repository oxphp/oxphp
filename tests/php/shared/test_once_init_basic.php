<?php
header('Content-Type: text/plain');

$o = new OxPHP\Shared\Once();

$calls = 0;
$v = $o->init(function() use (&$calls) { $calls++; return 42; });
if ($v !== 42) { echo "FAIL: init returns factory result\n"; exit; }
if ($calls !== 1) { echo "FAIL: factory ran $calls times (want 1)\n"; exit; }

$v2 = $o->init(function() use (&$calls) { $calls++; return 99; });
if ($v2 !== 42) { echo "FAIL: second init should return cached value\n"; exit; }
if ($calls !== 1) { echo "FAIL: factory ran again; count=$calls\n"; exit; }

$s = new OxPHP\Shared\Once();
if ($s->init(fn() => "hello") !== "hello") { echo "FAIL: string factory\n"; exit; }
if ($s->get() !== "hello") { echo "FAIL: string cached\n"; exit; }

$n = new OxPHP\Shared\Once();
if ($n->init(fn() => null) !== null) { echo "FAIL: null factory\n"; exit; }
if (!$n->isInitialized()) { echo "FAIL: null counts as initialised\n"; exit; }

echo "OK\n";
