<?php
/**
 * Channel — send throws TimeoutException when wait exceeds $timeout.
 */
header('Content-Type: text/plain');

$ch = new OxPHP\Shared\Channel(1);
$ch->send('fill');

$start = microtime(true);
$threw = false;
try {
    $ch->send('blocked', 0.1);
} catch (OxPHP\Shared\TimeoutException $e) {
    $threw = true;
}
$elapsed = microtime(true) - $start;

if (!$threw) { echo "FAIL: expected TimeoutException\n"; exit; }
// Allow generous tolerance: at least 50ms (half of 100ms target) and under 1s.
if ($elapsed < 0.05) { echo "FAIL: elapsed=$elapsed too short; expected >=0.05s\n"; exit; }
if ($elapsed >= 1.0)  { echo "FAIL: elapsed=$elapsed too long; expected <1s\n"; exit; }

echo "OK\n";
