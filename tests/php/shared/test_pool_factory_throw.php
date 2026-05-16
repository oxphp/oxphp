<?php
/**
 * Pool — factory throws during acquire.
 *
 * Verifies:
 *  - acquire() propagates the factory's own exception (class + message
 *    preserved), not wrapped in a Shared\* exception.
 *  - Budget is refunded: size/inUse return to 0 after the throw so the
 *    pool is immediately re-usable.
 *  - A subsequent acquire with a good factory succeeds normally.
 */

header('Content-Type: text/plain');

$calls = 0;
$pool = new OxPHP\Shared\Pool(
    function () use (&$calls): object {
        $calls++;
        if ($calls === 1) {
            throw new \RuntimeException("factory-bomb #$calls");
        }
        $r = new stdClass();
        $r->ok = true;
        return $r;
    },
    null,
    2,
);

$caught = null;
try {
    $pool->acquire();
} catch (\RuntimeException $e) {
    $caught = $e;
}

if ($caught === null) { echo "FAIL: factory throw must propagate\n"; exit; }
if ($caught->getMessage() !== 'factory-bomb #1') {
    echo "FAIL: wrong message: " . $caught->getMessage() . "\n"; exit;
}

if ($pool->count() !== 0)   { echo "FAIL: size must refund to 0, got " . $pool->count() . "\n"; exit; }
if ($pool->inUse() !== 0)  { echo "FAIL: inUse must refund to 0, got " . $pool->inUse() . "\n"; exit; }
if ($pool->idle() !== 0)   { echo "FAIL: idle must stay 0\n"; exit; }

// Second acquire must succeed — factory is re-invoked because no slot
// was minted on the first attempt.
$h = $pool->acquire();
if (!($h instanceof OxPHP\Shared\Pool\Handle)) { echo "FAIL: second acquire failed\n"; exit; }
if ($calls !== 2) { echo "FAIL: factory should have been called twice\n"; exit; }
if ($h->get()->ok !== true) { echo "FAIL: second acquire returned wrong resource\n"; exit; }

$pool->release($h);

echo "OK\n";
