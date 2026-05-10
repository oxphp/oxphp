<?php
/**
 * Cross-thread test — Atomic shared across ≥2 async workers via fetchAdd.
 * Mirrors test_counter_cross_thread.php but uses Atomic semantics
 * (returns prev) for the contention loop.
 */

use OxPHP\Shared\Atomic;

header('Content-Type: text/plain');

$a = new Atomic();
$n = 1000;
$promises = [];
for ($i = 0; $i < 4; $i++) {
    $promises[] = oxphp_async(function () use ($a, $n) {
        for ($j = 0; $j < $n; $j++) {
            $a->fetchAdd(1);
        }
    });
}
oxphp_async_await_all($promises);

$expected = 4 * $n;
$got = $a->load();
if ($got !== $expected) {
    echo "FAIL: expected $expected got $got\n"; exit;
}

echo "OK\n";
