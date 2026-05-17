<?php
/**
 * Channel — after close, recv drains remaining buffered items
 * before returning RecvResult::Closed.
 */
header('Content-Type: text/plain');

$ch = new OxPHP\Shared\Channel(4);
$ch->send('one');
$ch->send('two');
$ch->send('three');
$ch->close();

if ($ch->recv()->value() !== 'one')   { echo "FAIL: expected 'one'\n"; exit; }
if ($ch->recv()->value() !== 'two')   { echo "FAIL: expected 'two'\n"; exit; }
if ($ch->recv()->value() !== 'three') { echo "FAIL: expected 'three'\n"; exit; }

$r = $ch->recv();
if (!$r->isClosed()) { echo "FAIL: expected RecvResult::Closed after drain\n"; exit; }

$r2 = $ch->tryRecv();
if (!$r2->isClosed()) { echo "FAIL: tryRecv on closed+empty must be Closed\n"; exit; }

echo "OK\n";
