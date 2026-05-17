<?php
/**
 * Channel smoke test — construct / send / recv / tryRecv / close
 * under the Result-style API.
 */

header('Content-Type: text/plain');

$ch = new OxPHP\Shared\Channel(4);

$ch->send('a');
$ch->send('b');
if ($ch->recv()->value() !== 'a') { echo "FAIL: expected 'a' on first recv\n"; exit; }
if ($ch->recv()->value() !== 'b') { echo "FAIL: expected 'b' on second recv\n"; exit; }
if (!$ch->tryRecv()->isEmpty()) { echo "FAIL: tryRecv on empty must report Empty\n"; exit; }
$ch->close();

// send on closed must produce SendResult::Closed (no exception).
$sendResult = $ch->send('x');
if (!$sendResult->isClosed()) { echo "FAIL: send on closed must report Closed\n"; exit; }

// recv on closed + empty must produce RecvResult::Closed.
if (!$ch->recv()->isClosed()) { echo "FAIL: recv on closed+empty must report Closed\n"; exit; }

echo "OK\n";
