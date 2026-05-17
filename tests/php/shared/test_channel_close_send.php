<?php
/**
 * Channel — close(), then send → SendResult::Closed (no exception).
 * close() is idempotent (second call is a no-op).
 */
header('Content-Type: text/plain');

$ch = new OxPHP\Shared\Channel(2);
$ch->send(1);

$ch->close();
$ch->close(); // idempotent — second close must not throw

$result = $ch->send(2);
if (!$result->isClosed()) { echo "FAIL: send on closed channel must report Closed\n"; exit; }

// Channel should still report closed
if (!$ch->isClosed()) { echo "FAIL: isClosed() should be true after close\n"; exit; }

echo "OK\n";
