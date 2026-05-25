<?php
header('Content-Type: text/plain');

use OxPHP\Shared\Flag;
use OxPHP\Shared\Ordering;

$f = new Flag();
if ($f->load() !== false) { echo "FAIL: initial load\n"; exit; }

$f->store(true);
if ($f->load() !== true) { echo "FAIL: load after store\n"; exit; }

// swap returns the previous value
if ($f->swap(false) !== true) { echo "FAIL: swap returns prev=true\n"; exit; }
if ($f->load() !== false) { echo "FAIL: load after swap\n"; exit; }

// compareAndSet: hit then miss
if (!$f->compareAndSet(false, true)) { echo "FAIL: cas hit\n"; exit; }
if ($f->compareAndSet(false, true)) { echo "FAIL: cas miss\n"; exit; }
if ($f->load() !== true) { echo "FAIL: load after cas\n"; exit; }

// explicit ordering: Release store / Acquire load
$g = new Flag(false);
$g->store(true, Ordering::Release);
if ($g->load(Ordering::Acquire) !== true) { echo "FAIL: release/acquire\n"; exit; }

// constructor initial = true
$h = new Flag(true);
if ($h->load() !== true) { echo "FAIL: ctor initial true\n"; exit; }

echo "OK\n";
