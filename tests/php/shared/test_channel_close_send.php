<?php
/**
 * Channel — close(), then send → ClosedException.
 * close() is idempotent (second call is a no-op).
 */
header('Content-Type: text/plain');

$ch = new OxPHP\Shared\Channel(2);
$ch->send(1);

$ch->close();
$ch->close(); // idempotent — second close must not throw

$threw = false;
try {
    $ch->send(2);
} catch (OxPHP\Shared\ClosedException $e) {
    $threw = true;
}
if (!$threw) { echo "FAIL: send on closed must throw ClosedException\n"; exit; }

// Channel should still report closed
if (!$ch->isClosed()) { echo "FAIL: isClosed() should be true after close\n"; exit; }

echo "OK\n";
