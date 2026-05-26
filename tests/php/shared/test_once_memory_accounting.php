<?php
/**
 * OxPHP\Shared\Once memory accounting — Registry::memoryUsage() MUST
 * reflect the value stored in the cell, not just the empty skeleton.
 *
 * Reproducer: insert() books inner.mem_bytes() at creation time (≈16
 * bytes for an empty Once). After get_or_init() / try_set() stores a
 * large value, OnceInner::mem_bytes() returns 16 + value_size but
 * total_bytes is NEVER incremented past the original 16 — no
 * adjust_mem_bytes call follows the state transition. SHARED_MAX_BYTES
 * caps that operators rely on to bound RSS are silently bypassed by
 * lazily-initialised Once cells across the fleet.
 *
 * The test creates a Once, stores a value of known size, and asserts
 * memoryUsage() grew by at least that size minus a small slack.
 */
header('Content-Type: text/plain');

use OxPHP\Shared\Once;
use OxPHP\Shared\Registry;

// Suite-safe key so parallel re-runs do not collide.
$payload_size = 8192;
$payload = str_repeat('x', $payload_size);

$bytes_before = Registry::memoryUsage();
if (!is_int($bytes_before) || $bytes_before < 0) {
    echo "FAIL: memoryUsage must be non-negative int, got ", var_export($bytes_before, true), "\n"; exit;
}

// Anonymous Once — does not survive past the request, ensuring this
// test stays isolated from other Once tests in the same process.
$o = new Once();

$bytes_after_construct = Registry::memoryUsage();
$construct_delta = $bytes_after_construct - $bytes_before;
// Construction should add at least the skeleton bytes; allow concurrent
// churn on other Shared layers.
if ($construct_delta < 0) {
    echo "FAIL: memoryUsage shrank on Once construct (before=$bytes_before, after=$bytes_after_construct)\n"; exit;
}

// Store the payload through the cell.
$o->getOrInit(fn() => $payload);

$bytes_after_init = Registry::memoryUsage();
$init_delta = $bytes_after_init - $bytes_after_construct;

// Allow some slack for concurrent activity in the same process, but the
// stored 8 KiB payload MUST be reflected. The bug surfaces as
// $init_delta ≈ 0 (no growth) even though an 8 KiB string is now pinned.
// We require at least half of the payload size, which is impossible to
// reach without the accounting being adjusted post-init.
$min_expected = (int) ($payload_size * 0.5);

if ($init_delta < $min_expected) {
    echo "FAIL: Once::getOrInit storing ", $payload_size,
         " B did not grow memoryUsage by ≥ ", $min_expected, " B ",
         "(grew by $init_delta B; before_init=$bytes_after_construct, after_init=$bytes_after_init). ",
         "Once mem accounting is undercounted — SHARED_MAX_BYTES cap is bypassed.\n";
    exit;
}

echo "OK\n";
