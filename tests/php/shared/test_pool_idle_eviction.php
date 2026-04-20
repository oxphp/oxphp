<?php
/**
 * Idle-timeout eviction via $pool->evict().
 *
 * Exercises the synchronous eviction path end-to-end: a resource
 * sitting in the idle deque past its `idleTimeout` must be destroyed
 * when the user calls $pool->evict() on the owning thread.
 *
 * The background scheduler (src/plugins/ox_shared/eviction.rs) uses
 * the same drain primitive (`evict_stale_on_current_thread`) so a
 * passing $pool->evict() test indirectly proves the async path's
 * destroy plumbing is correct. The flag/request round-trip itself
 * is covered by Rust unit tests.
 */

header('Content-Type: text/plain');

// ── Scenario 1: stale slot is destroyed by evict() ─────────────────
$destroyedA = 0;
$poolA = new OxPHP\Shared\Pool(
    fn(): object => (object) ['n' => random_int(1, 1_000_000)],
    function (object $_r) use (&$destroyedA): void { $destroyedA++; },
    2,      // maxSize
    0.1,    // idleTimeout = 100ms
);

$ha = $poolA->acquire();
$poolA->release($ha);

if ($poolA->idle() !== 1) { echo "FAIL: expected 1 idle\n"; exit; }
if ($poolA->size() !== 1) { echo "FAIL: expected 1 size\n"; exit; }

// Sleep past idle_timeout. usleep is cooperative and does not yield
// the worker thread to another request — the slot stays parked in
// this thread's deque.
usleep(200_000);

$evicted = $poolA->evict();

if ($evicted !== 1)            { echo "FAIL: evict returned $evicted, expected 1\n"; exit; }
if ($destroyedA !== 1)          { echo "FAIL: destroy should run once, got $destroyedA\n"; exit; }
if ($poolA->size() !== 0)       { echo "FAIL: size should be 0 after eviction\n"; exit; }
if ($poolA->idle() !== 0)       { echo "FAIL: idle should be 0 after eviction\n"; exit; }

// ── Scenario 2: fresh slot survives evict() ────────────────────────
$destroyedB = 0;
$poolB = new OxPHP\Shared\Pool(
    fn(): object => new stdClass(),
    function (object $_r) use (&$destroyedB): void { $destroyedB++; },
    1,
    60.0,   // idleTimeout = 60s — nothing stale in a test run
);

$hb = $poolB->acquire();
$poolB->release($hb);

$evicted = $poolB->evict();

if ($evicted !== 0)             { echo "FAIL: fresh slot must not be evicted, got $evicted\n"; exit; }
if ($destroyedB !== 0)          { echo "FAIL: destroy must not run on fresh slot\n"; exit; }
if ($poolB->idle() !== 1)       { echo "FAIL: fresh slot must stay idle\n"; exit; }
if ($poolB->size() !== 1)       { echo "FAIL: fresh slot must stay counted\n"; exit; }

// ── Scenario 3: evict() stops at the first fresh slot ──────────────
// maxSize=2, two distinct resources, one aged, one fresh. evict()
// must remove only the aged one.
$destroyedC = 0;
$poolC = new OxPHP\Shared\Pool(
    fn(): object => (object) ['id' => random_int(1, 1_000_000)],
    function (object $_r) use (&$destroyedC): void { $destroyedC++; },
    2,
    0.1,    // 100ms
);

// Hold both slots simultaneously so the factory mints TWO resources.
$h1 = $poolC->acquire();
$h2 = $poolC->acquire();
$poolC->release($h1);

usleep(200_000); // Age the first slot only.
$poolC->release($h2); // Fresh back-of-deque slot.

if ($poolC->idle() !== 2) { echo "FAIL: expected 2 idle in poolC\n"; exit; }

$evicted = $poolC->evict();

if ($evicted !== 1)            { echo "FAIL: expected exactly 1 eviction, got $evicted\n"; exit; }
if ($destroyedC !== 1)          { echo "FAIL: exactly one destroy expected, got $destroyedC\n"; exit; }
if ($poolC->size() !== 1)       { echo "FAIL: fresh slot survives, size should be 1\n"; exit; }
if ($poolC->idle() !== 1)       { echo "FAIL: fresh slot survives, idle should be 1\n"; exit; }

echo "OK\n";
