<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('async_output_drain_unbuffered', 'async');

// With output_buffering=0 (this profile's default), a background task's echo
// bypasses the PHP output-buffer layer and lands directly in the Rust RESPONSE
// buffer via ub_write. The idle drain clears that Vec and counts the bytes, so
// the discarded-bytes counter must still grow — exercising the Rust-side path.
function scrape_discarded(): int {
    $m = @file_get_contents('http://127.0.0.1:9090/metrics');
    if (!is_string($m)) {
        return -1;
    }
    if (preg_match('/^oxphp_async_output_discarded_bytes_total\s+(\d+)$/m', $m, $mm)) {
        return (int) $mm[1];
    }
    return 0;
}

$blob = 4 * 1024 * 1024; // 4 MiB

$before = scrape_discarded();
$t->assertTrue('metrics endpoint reachable', $before >= 0);

$ok = oxphp_async_await(oxphp_async(function (int $n): bool {
    echo str_repeat('x', $n);
    return true;
}, $blob));
$t->assertTrue('task completed', $ok === true);

// Poll until the counter reflects our blob (the worker drains shortly after it
// goes idle; the counter update is visible on the next /metrics scrape).
$deadline = microtime(true) + 3.0;
$after = $before;
while (microtime(true) < $deadline) {
    $after = scrape_discarded();
    if ($after >= $before + $blob) {
        break;
    }
    usleep(50_000);
}
$t->assertTrue(
    "discarded counter grew by >= blob via Rust buffer (before={$before} after={$after})",
    $after >= $before + $blob
);

$t->done();
