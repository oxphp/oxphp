<?php
/**
 * Channel — recvTimeout on empty channel returns RecvResult::Timeout
 * (no exception).
 */
header('Content-Type: text/plain');

$ch = new OxPHP\Shared\Channel(1);

$start = microtime(true);
$got = $ch->recvTimeout(100);  // 100ms
$elapsed = microtime(true) - $start;

if (!$got->isTimeout()) {
    echo "FAIL: recvTimeout on empty must be Timeout, status=" . $got->status()->name . "\n";
    exit;
}
if ($elapsed < 0.05) { echo "FAIL: elapsed=$elapsed too short\n"; exit; }
if ($elapsed >= 1.0)  { echo "FAIL: elapsed=$elapsed too long\n"; exit; }

echo "OK\n";
