<?php
/** RecvResult API surface: isX / value / valueOr / status / non-Ok value() throws. */
use OxPHP\Shared\{Channel, SharedException};
use OxPHP\Shared\Channel\RecvStatus;

header('Content-Type: text/plain');

$ch = new Channel(4);
$ch->send(42);
$r = $ch->tryRecv();
if (!$r->isOk()) { echo "FAIL: should be Ok\n"; exit; }
if ($r->value() !== 42) { echo "FAIL: value should be 42\n"; exit; }
if ($r->valueOr(0) !== 42) { echo "FAIL: valueOr on Ok should return value\n"; exit; }
if ($r->status() !== RecvStatus::Ok) { echo "FAIL: status mismatch\n"; exit; }

$empty = $ch->tryRecv();
if (!$empty->isEmpty()) { echo "FAIL: should be Empty\n"; exit; }
if ($empty->valueOr('fallback') !== 'fallback') { echo "FAIL: valueOr on non-Ok\n"; exit; }

$threw = false;
try { $empty->value(); } catch (SharedException) { $threw = true; }
if (!$threw) { echo "FAIL: value() on Empty must throw SharedException\n"; exit; }

echo "OK\n";
