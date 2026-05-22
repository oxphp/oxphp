<?php
// getOrInit basic: runs once, caches; stored null is Ready not uninit.
header('Content-Type: text/plain');

use OxPHP\Shared\Once;
use OxPHP\Shared\Once\Status;

$o = new Once();

$calls = 0;
$v = $o->getOrInit(function() use (&$calls) { $calls++; return 42; });
if ($v !== 42) { echo "FAIL: getOrInit returns factory result\n"; exit; }
if ($calls !== 1) { echo "FAIL: factory ran $calls times (want 1)\n"; exit; }

$v2 = $o->getOrInit(function() use (&$calls) { $calls++; return 99; });
if ($v2 !== 42) { echo "FAIL: second getOrInit should return cached value\n"; exit; }
if ($calls !== 1) { echo "FAIL: factory ran again; count=$calls\n"; exit; }

$s = new Once();
if ($s->getOrInit(fn() => "hello") !== "hello") { echo "FAIL: string factory\n"; exit; }
if ($s->get() !== "hello") { echo "FAIL: string cached\n"; exit; }

$n = new Once();
if ($n->getOrInit(fn() => null) !== null) { echo "FAIL: null factory\n"; exit; }
if ($n->status() !== Status::Ready) { echo "FAIL: null counts as Ready\n"; exit; }
if ($n->get() !== null) { echo "FAIL: stored null reads back as null\n"; exit; }

echo "OK\n";
