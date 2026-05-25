<?php
/**
 * Atomic signed-integer edge cases: default-zero construction, negative
 * values, and i64 two's-complement wraparound at the int64 boundaries.
 * Each method maps onto AtomicI64::fetch_*, which wraps on overflow
 * rather than saturating or erroring.
 */

use OxPHP\Shared\Atomic;

header('Content-Type: text/plain');

// Default constructor initialises to 0.
$z = new Atomic();
if ($z->load() !== 0) { echo "FAIL: default ctor not 0\n"; exit; }

// Negative initial value round-trips.
$a = new Atomic(initial: -5);
if ($a->load() !== -5) { echo "FAIL: negative initial\n"; exit; }

// fetchAdd with a negative delta, descending further into the negatives.
if ($a->fetchAdd(-10) !== -5) { echo "FAIL: fetchAdd neg prev\n"; exit; }
if ($a->load() !== -15) { echo "FAIL: fetchAdd neg result\n"; exit; }

// fetchSub crossing zero into the negatives.
$a->store(3);
if ($a->fetchSub(8) !== 3) { echo "FAIL: fetchSub prev\n"; exit; }
if ($a->load() !== -5) { echo "FAIL: fetchSub below zero\n"; exit; }

// swap to a negative value, returning the previous.
if ($a->swap(-100) !== -5) { echo "FAIL: swap neg prev\n"; exit; }
if ($a->load() !== -100) { echo "FAIL: swap neg result\n"; exit; }

// compareAndSet across the sign boundary.
if (!$a->compareAndSet(-100, 100)) { echo "FAIL: cas neg->pos\n"; exit; }
if ($a->load() !== 100) { echo "FAIL: cas neg->pos result\n"; exit; }

// Upper-bound wraparound: PHP_INT_MAX + 1 == PHP_INT_MIN (two's complement).
$hi = new Atomic(initial: PHP_INT_MAX);
if ($hi->fetchAdd(1) !== PHP_INT_MAX) { echo "FAIL: wrap-hi prev\n"; exit; }
if ($hi->load() !== PHP_INT_MIN) { echo "FAIL: wrap-hi did not wrap\n"; exit; }

// Lower-bound wraparound: PHP_INT_MIN - 1 == PHP_INT_MAX.
$lo = new Atomic(initial: PHP_INT_MIN);
if ($lo->fetchSub(1) !== PHP_INT_MIN) { echo "FAIL: wrap-lo prev\n"; exit; }
if ($lo->load() !== PHP_INT_MAX) { echo "FAIL: wrap-lo did not wrap\n"; exit; }

echo "OK\n";
