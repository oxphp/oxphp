<?php
/**
 * Channel — after close, recv drains remaining buffered items
 * before returning null.
 */
header('Content-Type: text/plain');

$ch = new OxPHP\Shared\Channel(4);
$ch->send('one');
$ch->send('two');
$ch->send('three');
$ch->close();

if ($ch->recv() !== 'one')   { echo "FAIL: expected 'one'\n"; exit; }
if ($ch->recv() !== 'two')   { echo "FAIL: expected 'two'\n"; exit; }
if ($ch->recv() !== 'three') { echo "FAIL: expected 'three'\n"; exit; }
if ($ch->recv() !== null)    { echo "FAIL: expected null after drain\n"; exit; }

// Subsequent tryRecv on closed+empty must throw ClosedException
$threw = false;
try {
    $ch->tryRecv();
} catch (OxPHP\Shared\ClosedException $e) {
    $threw = true;
}
if (!$threw) { echo "FAIL: tryRecv on closed+empty must throw ClosedException\n"; exit; }

echo "OK\n";
