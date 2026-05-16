<?php
header('Content-Type: text/plain');

$f = new OxPHP\Shared\Flag();
if ($f->isSet() !== false) { echo "FAIL: initial isSet\n"; exit; }
if ($f->set() !== false) { echo "FAIL: set returns prev=false\n"; exit; }
if ($f->isSet() !== true) { echo "FAIL: isSet after set\n"; exit; }
if ($f->clear() !== true) { echo "FAIL: clear returns prev=true\n"; exit; }
if (!$f->compareAndSet(false, true)) { echo "FAIL: cas hit\n"; exit; }
if ($f->compareAndSet(false, true)) { echo "FAIL: cas miss\n"; exit; }
if ($f->exchange(false) !== true) { echo "FAIL: exchange\n"; exit; }

echo "OK\n";
