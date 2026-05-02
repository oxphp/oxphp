<?php
/**
 * Channel — recv on empty channel with timeout returns NULL (no exception).
 * recv returns null on all non-item outcomes including timeout. Asymmetric
 * with send (which throws TimeoutException).
 */
header('Content-Type: text/plain');

$ch = new OxPHP\Shared\Channel(1);

$start = microtime(true);
$got = $ch->recv(timeout: 0.1);
$elapsed = microtime(true) - $start;

if ($got !== null) { echo "FAIL: recv timeout should return null, got " . var_export($got, true) . "\n"; exit; }
if ($elapsed < 0.05) { echo "FAIL: elapsed=$elapsed too short\n"; exit; }
if ($elapsed >= 1.0)  { echo "FAIL: elapsed=$elapsed too long\n"; exit; }

echo "OK\n";
