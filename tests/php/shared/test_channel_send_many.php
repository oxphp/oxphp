<?php
/**
 * Channel::sendMany batched send semantics.
 */

header('Content-Type: text/plain');

// Basic batch send into an empty channel: all fit.
$ch = new OxPHP\Shared\Channel(100);
$sent = $ch->sendMany([1, 2, 3, 4, 5], 50);
if ($sent !== 5) {
    echo "FAIL: sendMany returned $sent, expected 5\n";
    exit;
}
if ($ch->count() !== 5) {
    echo "FAIL: count=" . $ch->count() . ", expected 5\n";
    exit;
}
if (count($ch) !== 5) { echo "FAIL: Countable count(\$ch)\n"; exit; }

// Drain one; the next batch should still fit fully (6 + 2 ≤ 100).
if ($ch->recv()->value() !== 1) {
    echo "FAIL: first recv should be 1\n";
    exit;
}
$sent2 = $ch->sendMany(['a', 'b'], 50);
if ($sent2 !== 2) {
    echo "FAIL: second sendMany returned $sent2, expected 2\n";
    exit;
}
if ($ch->count() !== 6) {
    echo "FAIL: count=" . $ch->count() . ", expected 6 (2..5 + 'a','b')\n";
    exit;
}

// Empty array → 0 sent, no exception.
$sent3 = $ch->sendMany([], 50);
if ($sent3 !== 0) {
    echo "FAIL: empty sendMany returned $sent3, expected 0\n";
    exit;
}

// Closed channel: sendMany returns 0 (no items sent) without throwing.
$closed = new OxPHP\Shared\Channel(4);
$closed->close();
$sent4 = $closed->sendMany([1, 2, 3], 50);
if ($sent4 !== 0) {
    echo "FAIL: sendMany on closed returned $sent4, expected 0\n";
    exit;
}

// Mixed types (int, string, bool, null, nested array) — all must serialize.
$ch2 = new OxPHP\Shared\Channel(10);
$sent5 = $ch2->sendMany([42, 'hello', true, null, [1, 2, 3]], 50);
if ($sent5 !== 5) {
    echo "FAIL: mixed-type sendMany returned $sent5, expected 5\n";
    exit;
}
if ($ch2->recv()->value() !== 42) { echo "FAIL: first mixed recv\n"; exit; }
if ($ch2->recv()->value() !== 'hello') { echo "FAIL: second mixed recv\n"; exit; }
if ($ch2->recv()->value() !== true) { echo "FAIL: third mixed recv\n"; exit; }
$nullRecv = $ch2->recv();
if (!$nullRecv->isOk() || $nullRecv->value() !== null) { echo "FAIL: fourth mixed recv (null payload)\n"; exit; }
if ($ch2->recv()->value() !== [1, 2, 3]) { echo "FAIL: fifth mixed recv (nested array)\n"; exit; }

echo "OK\n";
