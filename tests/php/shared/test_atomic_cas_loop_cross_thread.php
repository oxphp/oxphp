<?php
/**
 * Cross-thread compare-and-set retry loop: each of W async workers
 * performs N lock-free increments via a CAS loop (load → compute → CAS,
 * retry on miss). Exercises the contended CAS path, distinct from the
 * fetchAdd fast path in test_atomic_cross_thread.php.
 */

use OxPHP\Shared\Atomic;

header('Content-Type: text/plain');

$a = new Atomic(initial: 0);
$w = 4;
$n = 500;

$promises = [];
for ($i = 0; $i < $w; $i++) {
    $promises[] = oxphp_async(function () use ($a, $n) {
        for ($j = 0; $j < $n; $j++) {
            // Lock-free increment: re-read and retry until the swap wins.
            do {
                $cur = $a->load();
            } while (!$a->compareAndSet($cur, $cur + 1));
        }
    });
}
oxphp_async_await_all($promises);

$expected = $w * $n;
$got = $a->load();
if ($got !== $expected) {
    echo "FAIL: expected $expected got $got\n"; exit;
}

echo "OK\n";
