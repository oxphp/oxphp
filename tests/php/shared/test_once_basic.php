<?php
// Once smoke test — get() throws on uninit; status machine; trySet.
header('Content-Type: text/plain');

use OxPHP\Shared\Once;
use OxPHP\Shared\Once\Status;

$o = new Once();
if ($o->status() !== Status::Uninitialized) { echo "FAIL: initial status\n"; exit; }

$threw = false;
try { $o->get(); } catch (OxPHP\Shared\UninitializedException $e) { $threw = true; }
if (!$threw) { echo "FAIL: get() must throw on uninit\n"; exit; }

if (!$o->trySet(42)) { echo "FAIL: first trySet should win\n"; exit; }
if ($o->trySet(99)) { echo "FAIL: second trySet should lose\n"; exit; }
if ($o->status() !== Status::Ready) { echo "FAIL: status after set\n"; exit; }
if ($o->get() !== 42) { echo "FAIL: wrong cached value\n"; exit; }

// String variant
$o2 = new Once();
if (!$o2->trySet("singleton")) { echo "FAIL: string trySet\n"; exit; }
if ($o2->get() !== "singleton") { echo "FAIL: string get\n"; exit; }

// Bool variant
$o3 = new Once();
if (!$o3->trySet(true)) { echo "FAIL: bool trySet\n"; exit; }
if ($o3->get() !== true) { echo "FAIL: bool get\n"; exit; }

// Null variant — stored null is Ready, distinguishable from uninit via status().
$o4 = new Once();
if (!$o4->trySet(null)) { echo "FAIL: null trySet\n"; exit; }
if ($o4->status() !== Status::Ready) { echo "FAIL: null counts as Ready\n"; exit; }
if ($o4->get() !== null) { echo "FAIL: null get\n"; exit; }
if ($o4->trySet(1)) { echo "FAIL: second write on null-initialized\n"; exit; }

if (!is_int($o->id())) { echo "FAIL: id type\n"; exit; }

echo "OK\n";
