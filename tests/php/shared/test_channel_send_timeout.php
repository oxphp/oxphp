<?php
/**
 * Channel — sendTimeout returns SendResult::Timeout when wait exceeds budget.
 */
header('Content-Type: text/plain');

$ch = new OxPHP\Shared\Channel(1);
$ch->send('fill');

$start = microtime(true);
$result = $ch->sendTimeout('blocked', 100);  // 100ms
$elapsed = microtime(true) - $start;

if (!$result->isTimeout()) {
    echo "FAIL: expected SendResult::Timeout, got " . $result->status()->name . "\n";
    exit;
}
// Allow generous tolerance: at least 50ms (half of 100ms target) and under 1s.
if ($elapsed < 0.05) { echo "FAIL: elapsed=$elapsed too short; expected >=0.05s\n"; exit; }
if ($elapsed >= 1.0)  { echo "FAIL: elapsed=$elapsed too long; expected <1s\n"; exit; }

echo "OK\n";
