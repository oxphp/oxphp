<?php
/** SendResult API surface: isOk / isFull / isClosed / status. */
use OxPHP\Shared\Channel;
use OxPHP\Shared\Channel\SendStatus;

header('Content-Type: text/plain');

$ch = new Channel(1);
$r = $ch->trySend('a');
if (!$r->isOk() || $r->status() !== SendStatus::Ok) { echo "FAIL: Ok\n"; exit; }

$full = $ch->trySend('b');
if (!$full->isFull() || $full->status() !== SendStatus::Full) { echo "FAIL: Full\n"; exit; }

$ch->recv()->value();  // drain
$ch->close();
$closed = $ch->send('x');
if (!$closed->isClosed() || $closed->status() !== SendStatus::Closed) { echo "FAIL: Closed\n"; exit; }

echo "OK\n";
