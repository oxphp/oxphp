<?php
/**
 * Channel — tryRecv semantics:
 *   - empty + open → RecvResult::Empty
 *   - item present → RecvResult::Ok with value
 *   - closed + empty → RecvResult::Closed
 */

header('Content-Type: text/plain');

$ch = new OxPHP\Shared\Channel(4);

if (!$ch->tryRecv()->isEmpty()) { echo "FAIL: empty+open tryRecv should be Empty\n"; exit; }

$ch->send('x');
$got = $ch->tryRecv();
if (!$got->isOk() || $got->value() !== 'x') { echo "FAIL: tryRecv should return 'x'\n"; exit; }
if (!$ch->tryRecv()->isEmpty()) { echo "FAIL: drained tryRecv should be Empty\n"; exit; }

$ch->close();
if (!$ch->tryRecv()->isClosed()) { echo "FAIL: tryRecv on closed+empty should be Closed\n"; exit; }

echo "OK\n";
