<?php
/**
 * Pool `with()` API — acquire + body($resource) + release,
 * including exception-safety (release runs even when body throws).
 */

header('Content-Type: text/plain');

$pool = new OxPHP\Shared\Pool(
    function (): object {
        $r = new stdClass();
        $r->hits = 0;
        return $r;
    },
    null, // destroy
    1,    // maxSize
);

$ret = $pool->with(function (object $r): int {
    $r->hits++;
    return 42;
});
if ($ret !== 42)            { echo "FAIL: with() returns body's result, got " . var_export($ret, true) . "\n"; exit; }
if ($pool->inUse() !== 0)   { echo "FAIL: with() must release on success\n"; exit; }
if ($pool->idle() !== 1)    { echo "FAIL: slot should be idle after with()\n"; exit; }

// Second with() reuses the slot (factory runs once total).
$pool->with(function (object $r): void {
    $r->hits++; // Operates on the same underlying object.
});

// Body throw path: release MUST still run.
$caught = false;
try {
    $pool->with(function (object $r): void {
        throw new \RuntimeException('body-bomb');
    });
} catch (\RuntimeException $e) {
    if ($e->getMessage() === 'body-bomb') {
        $caught = true;
    }
}
if (!$caught) { echo "FAIL: with() must propagate body throw\n"; exit; }
if ($pool->inUse() !== 0) { echo "FAIL: with() must release even on body throw\n"; exit; }

echo "OK\n";
