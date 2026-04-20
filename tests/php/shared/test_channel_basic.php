<?php
/**
 * Channel smoke test — construct / send / recv / tryRecv / close.
 */

header('Content-Type: text/plain');

$ch = new OxPHP\Shared\Channel(4);

$ch->send('a');
$ch->send('b');
if ($ch->recv() !== 'a') { echo "FAIL: expected 'a' on first recv\n"; exit; }
if ($ch->recv() !== 'b') { echo "FAIL: expected 'b' on second recv\n"; exit; }
if ($ch->tryRecv() !== null) { echo "FAIL: tryRecv on empty should be null\n"; exit; }
$ch->close();

// send on closed must throw ClosedException
$threw = false;
try {
    $ch->send('x');
} catch (OxPHP\Shared\ClosedException $e) {
    $threw = true;
}
if (!$threw) { echo "FAIL: send on closed must throw ClosedException\n"; exit; }

// recv on closed + empty returns null (does NOT throw)
if ($ch->recv() !== null) { echo "FAIL: recv on closed+empty must return null\n"; exit; }

echo "OK\n";
