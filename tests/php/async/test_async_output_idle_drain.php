<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('async_output_idle_drain', 'async');

/** Scrape the discarded-bytes counter; -1 = endpoint unreachable, 0 = line absent. */
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

// A background task echoes a large blob. It has no client, so the bytes pile
// up in the worker's shared PHP output buffer until the idle drain reclaims them.
$p = oxphp_async(function (int $n): bool {
    echo str_repeat('x', $n);
    return true;
}, $blob);
$t->assertTrue('task completed', oxphp_async_await($p) === true);

// The worker is now idle; the next driver iteration drains the buffer and adds
// the discarded byte count to the counter. Poll until it reflects our blob.
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
    "discarded counter grew by >= blob (before={$before} after={$after})",
    $after >= $before + $blob
);

// The default output buffer must be restored after the drain: a second tiny
// task still runs and returns correctly (no corruption / no output bleed).
$p2 = oxphp_async(fn(): int => 42);
$t->assertSame('second task works after drain', oxphp_async_await($p2), 42);

$t->done();
