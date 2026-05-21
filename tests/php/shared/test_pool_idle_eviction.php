<?php
/**
 * Manual eviction — `$pool->evict()` force-evicts ALL idle slots now,
 * regardless of idleTimeoutMs, and runs $destroy for each. In-use slots
 * are untouched.
 *
 * This is the operational "flush idle now" escape hatch (downstream
 * restarted → drop idle resources so the next acquire mints fresh ones).
 * Age-based background eviction is a separate path covered by Rust unit
 * tests in src/plugins/ox_shared/types/pool.rs.
 */

header('Content-Type: text/plain');

// ── Scenario 1: evict() flushes an idle slot and runs $destroy ─────
$destroyedA = 0;
$poolA = new OxPHP\Shared\Pool(
    fn(): object => (object) ['n' => random_int(1, 1_000_000)],
    function (object $_r) use (&$destroyedA): void { $destroyedA++; },
    2,           // maxSize
    300_000,     // idleTimeoutMs — long; evict() ignores it
);

$ha = $poolA->acquire();
$ha->release();

$s = $poolA->stats();
if ($s->idle() !== 1) { echo "FAIL: expected 1 idle\n"; exit; }
if ($s->size() !== 1) { echo "FAIL: expected 1 size\n"; exit; }

$evicted = $poolA->evict();
if ($evicted !== 1)            { echo "FAIL: evict returned $evicted, expected 1\n"; exit; }
if ($destroyedA !== 1)          { echo "FAIL: destroy should run once, got $destroyedA\n"; exit; }
$s = $poolA->stats();
if ($s->size() !== 0)       { echo "FAIL: size should be 0 after eviction\n"; exit; }
if ($s->idle() !== 0)       { echo "FAIL: idle should be 0 after eviction\n"; exit; }

// ── Scenario 2: evict() force-evicts even a freshly released slot ──
// Unlike age-based eviction, manual evict() does not spare recently
// released slots.
$destroyedB = 0;
$poolB = new OxPHP\Shared\Pool(
    fn(): object => new stdClass(),
    function (object $_r) use (&$destroyedB): void { $destroyedB++; },
    1,
    300_000,
);

$hb = $poolB->acquire();
$hb->release(); // fresh idle slot, just released

$evicted = $poolB->evict();
if ($evicted !== 1)             { echo "FAIL: evict() must flush even fresh idle, got $evicted\n"; exit; }
if ($destroyedB !== 1)          { echo "FAIL: destroy must run on the flushed slot\n"; exit; }
$s = $poolB->stats();
if ($s->idle() !== 0)       { echo "FAIL: idle should be 0 after evict()\n"; exit; }
if ($s->size() !== 0)       { echo "FAIL: size should be 0 after evict()\n"; exit; }

// ── Scenario 3: evict() flushes ALL idle, leaves in-use untouched ──
$destroyedC = 0;
$poolC = new OxPHP\Shared\Pool(
    fn(): object => (object) ['id' => random_int(1, 1_000_000)],
    function (object $_r) use (&$destroyedC): void { $destroyedC++; },
    3,
    300_000,
);

// Mint three distinct resources by holding all three at once.
$h1 = $poolC->acquire();
$h2 = $poolC->acquire();
$h3 = $poolC->acquire();
$h1->release();
$h2->release(); // two idle, h3 still in use

$s = $poolC->stats();
if ($s->idle() !== 2)  { echo "FAIL: expected 2 idle in poolC, got {$s->idle()}\n"; exit; }
if ($s->inUse() !== 1) { echo "FAIL: expected 1 in-use in poolC, got {$s->inUse()}\n"; exit; }

$evicted = $poolC->evict();
if ($evicted !== 2)            { echo "FAIL: evict() must flush all 2 idle, got $evicted\n"; exit; }
if ($destroyedC !== 2)          { echo "FAIL: two destroys expected, got $destroyedC\n"; exit; }
$s = $poolC->stats();
if ($s->inUse() !== 1)      { echo "FAIL: in-use slot must survive evict(), got {$s->inUse()}\n"; exit; }
if ($s->idle() !== 0)       { echo "FAIL: all idle must be gone, got {$s->idle()}\n"; exit; }

$h3->release();

echo "OK\n";
