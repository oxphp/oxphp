<?php
/**
 * Channel::recvMany batched recv semantics.
 */

header('Content-Type: text/plain');

$ch = new OxPHP\Shared\Channel(100);
$ch->sendMany([1, 2, 3, 4, 5], 50);

// max > 0: pull exactly that many.
$got = $ch->recvMany(3, 50);
if (count($got) !== 3) {
    echo "FAIL: recvMany(3) returned " . count($got) . "\n";
    exit;
}
if ($got !== [1, 2, 3]) {
    echo "FAIL: recvMany order got " . var_export($got, true) . "\n";
    exit;
}

// Drain whatever is currently in the channel within a short budget.
$rest = $ch->recvMany(10, 50);
if ($rest !== [4, 5]) {
    echo "FAIL: drain got " . var_export($rest, true) . "\n";
    exit;
}

// Empty + open + short timeout → empty array (no exception).
$empty = $ch->recvMany(5, 50);
if ($empty !== []) {
    echo "FAIL: expected [] on empty+timeout, got " . var_export($empty, true) . "\n";
    exit;
}

// Closed+empty channel: recvMany returns [] regardless of max.
$ch->close();
$after_close = $ch->recvMany(10, 50);
if ($after_close !== []) {
    echo "FAIL: recvMany on closed+empty got " . var_export($after_close, true) . "\n";
    exit;
}

// Partial batch: 3 items buffered, ask for 10 with short timeout →
// should get all 3 before returning (deadline lets recv_blocking see
// each item quickly via its internal poll).
$ch2 = new OxPHP\Shared\Channel(100);
$ch2->sendMany(['a', 'b', 'c'], 50);
$partial = $ch2->recvMany(10, 100);
if ($partial !== ['a', 'b', 'c']) {
    echo "FAIL: partial batch got " . var_export($partial, true) . "\n";
    exit;
}

echo "OK\n";
