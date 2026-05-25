<?php
/**
 * Concurrent bitset: W async workers each own one distinct bit and set
 * it via fetchOr; the final mask must have all W low bits set. fetchOr
 * is a read-modify-write op — this checks it composes correctly under
 * real cross-thread contention (single-threaded coverage lives in
 * test_atomic_basic.php).
 */

use OxPHP\Shared\Atomic;

header('Content-Type: text/plain');

$bits = new Atomic(initial: 0);
$w = 16;

$promises = [];
for ($i = 0; $i < $w; $i++) {
    $promises[] = oxphp_async(function () use ($bits, $i) {
        // Repeat the OR: it is idempotent, so re-setting one worker's bit
        // must never clear another worker's concurrently-set bit.
        for ($k = 0; $k < 100; $k++) {
            $bits->fetchOr(1 << $i);
        }
    });
}
oxphp_async_await_all($promises);

$expected = (1 << $w) - 1; // all W low bits set
$got = $bits->load();
if ($got !== $expected) {
    echo "FAIL: expected $expected got $got\n"; exit;
}

echo "OK\n";
