<?php
/**
 * Pool\Handle release semantics — idempotent release(), get() after
 * release throws StaleHandleException, and RAII auto-release on scope
 * exit (including stack unwind on exception).
 */
header('Content-Type: text/plain');

$pool = new OxPHP\Shared\Pool(fn(): object => new stdClass(), null, 1);

// Idempotent release: a second release() is a no-op (must not throw).
$h = $pool->acquire();
$h->release();
$h->release();

// get() after release throws StaleHandleException.
$threw = false;
try {
    $h->get();
} catch (OxPHP\Shared\StaleHandleException $e) {
    $threw = true;
}
if (!$threw) { echo "FAIL: get() after release must throw StaleHandleException\n"; exit; }

// The slot returned to the pool: it is idle and acquirable again.
if ($pool->stats()->idle() !== 1) { echo "FAIL: released slot must be idle\n"; exit; }

// RAII: a Handle that falls out of scope returns its slot, even when the
// scope exits via an exception (stack unwind runs __destruct).
function leaky(OxPHP\Shared\Pool $p): void {
    $x = $p->acquire();
    throw new RuntimeException("boom"); // $x destructs during unwind
}
try { leaky($pool); } catch (RuntimeException $e) {}

$after = $pool->tryAcquire(); // slot must have come back
if (!$after instanceof OxPHP\Shared\Pool\Handle) { echo "FAIL: RAII release on exception failed\n"; exit; }
$after->release();

echo "OK\n";
