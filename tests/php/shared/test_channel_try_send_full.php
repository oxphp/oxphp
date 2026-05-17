<?php
/**
 * Channel — trySend returns SendResult::Ok / Full.
 */

header('Content-Type: text/plain');

$ch = new OxPHP\Shared\Channel(1);

$r1 = $ch->trySend(1);
if (!$r1->isOk())   { echo "FAIL: first trySend should be Ok\n"; exit; }
$r2 = $ch->trySend(2);
if (!$r2->isFull()) { echo "FAIL: second trySend on full should be Full\n"; exit; }

if ($ch->recv()->value() !== 1) { echo "FAIL: expected 1 from recv\n"; exit; }
$r3 = $ch->trySend(3);
if (!$r3->isOk())   { echo "FAIL: trySend after drain should be Ok\n"; exit; }

echo "OK\n";
