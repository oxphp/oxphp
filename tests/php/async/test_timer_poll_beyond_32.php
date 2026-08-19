<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('timer_poll_beyond_32', 'async');

// More sleep timers expiring in one poll window than the scheduler's 32-slot
// poll buffer holds. Every sleeper must still wake: the expired tail that does
// not fit in one poll stays registered and is delivered by the next one.
//
// Making 33+ timers expire in a single window is arranged, not raced: the
// sleepers park first (0.2s each), then one CPU-bound task pins the single
// async worker's driver loop for ~0.5s. No tick runs while it spins, so every
// sleeper's deadline has passed by the time the next timer poll happens — that
// one poll sees all 48 expired at once. Each sleeper returns its wake-up
// timestamp so the premise is asserted, not assumed: a host slow enough to
// stretch the dispatch past the sleep duration would wake sleepers before the
// spin and quietly shrink the batch below the buffer size.
$sleepers = [];
for ($i = 0; $i < 48; $i++) {
    $sleepers[] = oxphp_async(function () use ($i): array {
        oxphp_sleep(0.2);
        return [$i, microtime(true)];
    });
}

$busy = oxphp_async(function (): array {
    $end = hrtime(true) + 500_000_000; // 0.5s of wall clock, never suspends
    while (hrtime(true) < $end) {
        // spin: hold the driver thread so no scheduler tick runs
    }
    return ['busy-done', microtime(true)];
});

$start = microtime(true);
$threw = false;
$msg = '';
$results = [];
try {
    $results = oxphp_async_await_all(array_merge($sleepers, [$busy]), 5.0);
} catch (\Throwable $e) {
    $threw = true;
    $msg = get_class($e) . ': ' . $e->getMessage();
}
$elapsed = microtime(true) - $start;

$t->assertFalse('await_all completed without timing out' . ($threw ? " ($msg)" : ''), $threw);

// Both halves of the guarantee. Delivered: every sleeper woke and returned its
// own index — a lost timer means a missing result and a TimeoutException above.
// Results are separated by shape rather than by position: await_all's ordering
// is not asserted anywhere else and is not what this test is about.
$wakeByIndex = [];
$busyEnd = null;
foreach ($results as $r) {
    if (!is_array($r)) {
        continue;
    }
    if (($r[0] ?? null) === 'busy-done') {
        $busyEnd = $r[1];
    } else {
        $wakeByIndex[$r[0]] = $r[1];
    }
}
$indices = array_keys($wakeByIndex);
sort($indices);
$t->assertSame('all 48 sleepers woke and returned their indices', $indices, range(0, 47));
$t->assertNotNull('the CPU-bound task finished too', $busyEnd);

// The premise held: at least 33 sleepers were still parked when the spin
// ended, so the first post-spin poll faced more expired timers than its
// buffer holds. Sleepers woken after the spin necessarily expired inside it —
// every deadline (park + 0.2s) lands before the spin's end (start + 0.5s).
$sleptThrough = count(array_filter($wakeByIndex, fn (float $wake): bool => $wake >= $busyEnd));
$t->assertGreaterThan('at least 33 sleepers slept through the pinned window (one poll saw >32 expired)', $sleptThrough, 32);

// Bounded: the tail woke on the polls right after the spin ended (~0.5s), not
// rescued late by anything else. The only later rescue is the 5s await budget
// itself (which the first assertion already rules out), so the bound needs
// only to stay clearly under it — 4s leaves slack for a loaded host.
$t->assertLessThan('everything completed promptly after the pinned window', $elapsed, 4.0);

$t->meta('elapsed_s', round($elapsed, 3));
$t->meta('slept_through', $sleptThrough);

$t->done();
