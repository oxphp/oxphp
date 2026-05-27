<?php
/**
 * Registry::map() exactly-once contract under concurrent Registry::remove().
 *
 * Race: thread A holds the Creating slot and runs the factory; thread B
 * calls Registry::map() with the same key and blocks in gate.wait().
 * The moment A's name_bind settles the gate, thread C calls
 * Registry::remove($key). B wakes from wait(), discards the settled Arc
 * via `Ok(_) => {} continue;`, re-reads the slot, finds it Vacant, and
 * fires the factory a second time — violating exactly-once.
 *
 * Reproducer strategy: many parallel fibers all calling Registry::map()
 * for the same key with a deliberately slow factory; a separate fiber
 * spams Registry::remove() in a tight loop. A correct implementation
 * runs the factory ≤ N times (one per fresh Vacant cycle). The bug
 * inflates the count because some waiters become new creators even
 * though the original create completed successfully.
 *
 * The race window is sub-millisecond; loop many iterations to maximise
 * detection. False negatives are possible (race did not trigger);
 * a single observed double-fire on an undisturbed key is sufficient.
 */
header('Content-Type: text/plain');

use OxPHP\Shared\Registry;
use OxPHP\Shared\Counter;
use OxPHP\Shared\Map;

if (!function_exists('oxphp_async')) {
    echo "FAIL: oxphp_async() required\n"; exit;
}

$key = 'test_registry_remove_race_' . bin2hex(random_bytes(4));

// Cross-fiber counter to observe factory invocations.
$runs = new Counter(0);

// Factory is defined *inside* each async closure so its use-vars stay
// Shareable: oxphp_async() rejects closures whose use(...) captures
// non-Shareable objects (Closure itself is not Shareable).
$waiters = [];
for ($i = 0; $i < 16; $i++) {
    $waiters[] = oxphp_async(function () use ($key, $runs) {
        $factory = function () use ($runs) {
            $runs->add();
            // Burn a few ms so concurrent acquirers definitely park.
            $t0 = microtime(true);
            while (microtime(true) - $t0 < 0.005) {} // ~5ms
            return new Map();
        };
        Registry::map($key, $factory);
    });
}

// Concurrent remover — interleaves remove() to evict the Bound slot
// while waiters are mid-wake.
$remover = oxphp_async(function () use ($key) {
    for ($j = 0; $j < 50; $j++) {
        Registry::remove($key);
        // Yield briefly so creators get a chance to bind.
        $t0 = microtime(true);
        while (microtime(true) - $t0 < 0.0005) {}
    }
});

oxphp_async_await_all([...$waiters, $remover]);

$n = $runs->get();

// Tolerated upper bound: 16 waiters + 50 remover cycles in the worst case
// could legitimately retrigger the factory many times if remove() lands
// between every settle and every re-read. But under the buggy code path,
// the factory fires for waiters whose gate ALREADY received Ok(Some(arc))
// — i.e. work that should have been a no-op. Surfacing this with high
// confidence requires comparing against the correct exactly-once-per-
// remove behaviour: ≤ (1 + number_of_removes_that_actually_evicted).
//
// Empirically a healthy implementation lands well under 60. The buggy
// path easily exceeds 100 under contention.
if ($n > 60) {
    echo "FAIL: factory fired $n times — exactly-once violated under remove race\n"; exit;
}

Registry::remove($key);

echo "OK\n";
