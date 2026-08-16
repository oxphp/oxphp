<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('async_cap_error_paths', 'asynccap');

// Profile asynccap: ASYNC_WORKERS=1, ASYNC_MAX_FIBERS=2 → process-global
// in-flight cap = 2. A dispatched task holds one permit from dispatch until it
// SETTLES, and that permit must be returned however the task settles — not only
// on a normal return but also on an uncaught throw, a fatal die(), or
// cancellation by an awaiter that gave up. A permit leaked on an error/cancel
// path would shrink the cap permanently. The only other cap test exercises
// success-only tasks, so this guards the error/panic/cancel return paths.
//
// Each phase dispatches MORE tasks than the cap, one at a time, draining each
// before dispatching the next. Serial dispatch needs only a single free slot,
// and at most the immediately-preceding task can still be settling, so the
// cap=2 headroom keeps every dispatch race-free. If a path leaked, permits
// would accumulate and the (cap+1)-th dispatch would be rejected with
// AsyncException instead of returning a promise id.

const CAP = 2;
const ITER = 4; // > CAP, so a leak is rejected by the 3rd dispatch

// Drive ITER error tasks serially through dispatch + drain. Returns
// [dispatched, settled]: how many dispatches were accepted (a leak rejects
// past the cap) and how many awaits surfaced the $expected failure class.
$drive = function (\Closure $task, string $expected, float $awaitTimeout): array {
    $dispatched = 0;
    $settled = 0;
    for ($i = 0; $i < ITER; $i++) {
        try {
            $id = oxphp_async($task);
        } catch (\OxPHP\Async\AsyncException $e) {
            // Dispatch rejected = capacity exhausted = a prior permit leaked.
            break;
        }
        $dispatched++;
        try {
            oxphp_async_await($id, $awaitTimeout);
        } catch (\Throwable $e) {
            if ($e instanceof $expected) {
                $settled++;
            }
        }
    }
    return [$dispatched, $settled];
};

// ── (a) tasks that throw an uncaught exception ────────────────────────
[$d, $s] = $drive(
    fn (): int => throw new \RuntimeException('boom'),
    \OxPHP\Async\AsyncException::class,
    3.0
);
$t->assertSame('throwing tasks: all dispatches accepted (permit returned on throw)', $d, ITER);
$t->assertSame('throwing tasks: every await surfaced AsyncException', $s, ITER);

// ── (b) tasks that fatal via die() ────────────────────────────────────
[$d, $s] = $drive(
    function (): void { die('fatal'); },
    \OxPHP\Async\AsyncException::class,
    3.0
);
$t->assertSame('die() tasks: all dispatches accepted (permit returned on fatal)', $d, ITER);
$t->assertSame('die() tasks: every await surfaced AsyncException', $s, ITER);

// ── (c) CPU-bound tasks cancelled by the awaiter giving up ────────────
// The awaiter times out and abandons the task; the worker interrupts the
// still-running fiber, which unwinds and drains — returning the permit.
[$d, $s] = $drive(
    function (): int {
        $x = 0;
        while (true) {
            $x++;
        }
    },
    \OxPHP\Async\TimeoutException::class,
    0.3
);
$t->assertSame('cancelled tasks: all dispatches accepted (permit returned on cancel)', $d, ITER);
$t->assertSame('cancelled tasks: every awaiter timed out', $s, ITER);

// ── (d) full capacity restored after the error/cancel churn ───────────
// Stronger than the per-phase serial check, and a guard against any cumulative
// cross-phase leak: hold CAP tasks concurrently to prove all CAP slots are
// simultaneously free. A cancelled task drains asynchronously after its awaiter
// gave up, so poll (bounded) until both slots are free rather than assuming an
// instant release. The blocking usleep keeps the first task in flight while the
// second is dispatched, so success proves concurrency, not sequential reuse.
$deadline = microtime(true) + 3.0;
$restored = false;
while (microtime(true) < $deadline) {
    $ids = [];
    $full = true;
    for ($j = 0; $j < CAP; $j++) {
        try {
            $ids[] = oxphp_async(function (): int {
                usleep(150000);
                return 7;
            });
        } catch (\OxPHP\Async\AsyncException $e) {
            $full = false;
            break;
        }
    }
    foreach ($ids as $id) {
        try {
            oxphp_async_await($id, 3.0);
        } catch (\Throwable $e) {
            // ignore — we only care that the slots were obtainable
        }
    }
    if ($full && count($ids) === CAP) {
        $restored = true;
        break;
    }
    usleep(50000);
}
$t->assertTrue('full capacity (CAP concurrent) restored after all error/cancel paths', $restored);

$t->done();
