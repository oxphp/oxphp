<?php
/**
 * Channel — trySend returns false when full, true on success.
 */

header('Content-Type: text/plain');

$ch = new OxPHP\Shared\Channel(1);

if ($ch->trySend(1) !== true) { echo "FAIL: first trySend should return true\n"; exit; }
if ($ch->trySend(2) !== false) { echo "FAIL: second trySend on full should return false\n"; exit; }

if ($ch->recv() !== 1) { echo "FAIL: expected 1 from recv\n"; exit; }
if ($ch->trySend(3) !== true) { echo "FAIL: trySend after drain should return true\n"; exit; }

echo "OK\n";
