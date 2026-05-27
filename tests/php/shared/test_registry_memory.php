<?php
/**
 * memoryUsage() and count() report the WHOLE Shared layer (named +
 * anonymous). Both must grow when an entry is created and not shrink
 * unexpectedly during this request.
 *
 * Note: these numbers are transient and shared across all in-flight
 * requests; we only assert relative growth, not absolute values.
 */

use OxPHP\Shared\Registry;
use OxPHP\Shared\Map;

header('Content-Type: text/plain');

$key = 'test_registry_memory_' . bin2hex(random_bytes(4));

$bytes_before   = Registry::memoryUsage();
$entries_before = Registry::count();

if (!is_int($bytes_before) || $bytes_before < 0) { echo "FAIL: memoryUsage must be non-negative int\n"; exit; }
if (!is_int($entries_before) || $entries_before < 0) { echo "FAIL: count must be non-negative int\n"; exit; }

$m = Registry::map($key, fn() => new Map());

$bytes_after   = Registry::memoryUsage();
$entries_after = Registry::count();

if ($bytes_after   <= $bytes_before)   { echo "FAIL: memoryUsage did not grow (before=$bytes_before, after=$bytes_after)\n"; exit; }
if ($entries_after !== $entries_before + 1) {
    // Allow drift due to concurrent anonymous churn, but require at least +1.
    if ($entries_after < $entries_before + 1) {
        echo "FAIL: count did not grow by at least 1 (before=$entries_before, after=$entries_after)\n"; exit;
    }
}

Registry::remove($key);

echo "OK\n";
