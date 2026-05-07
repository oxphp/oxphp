<?php
/**
 * Pool — unified timeout contract on acquire():
 *   null=forever, 0.0=try, INF=forever, NaN→Type, negative→Type.
 */
header('Content-Type: text/plain');

$pool = new OxPHP\Shared\Pool(
    fn(): object => new stdClass(),
    null,
    1,       // maxSize → only one slot
);

$held = $pool->acquire(); // null implicit → forever (immediate, pool empty).

// 0.0 = try → acquire on saturated pool throws TimeoutException without blocking.
$start = microtime(true);
$threw = false;
try {
    $pool->acquire(0.0);
} catch (OxPHP\Shared\TimeoutException $e) {
    $threw = true;
}
$elapsed = microtime(true) - $start;
if (!$threw) { echo "FAIL: acquire(0.0) on saturated must throw\n"; exit; }
if ($elapsed >= 0.05) { echo "FAIL: acquire(0.0) must be immediate, elapsed=$elapsed\n"; exit; }

// NaN, negative → TypeException.
foreach ([NAN, -0.5] as $bad) {
    $caught = null;
    try {
        $pool->acquire($bad);
    } catch (OxPHP\Shared\TypeException $e) {
        $caught = $e;
    }
    if ($caught === null) { echo "FAIL: acquire(" . var_export($bad, true) . ") must throw TypeException\n"; exit; }
}

$pool->release($held);

// INF = forever — release after a delay, acquire(INF) gets it.
$held2 = $pool->acquire();
$releaser = oxphp_async(function () use ($pool, $held2) {
    usleep(80_000);
    $pool->release($held2);
});
$got = $pool->acquire(INF);
if (!$got instanceof OxPHP\Shared\Pool\Handle) { echo "FAIL: acquire(INF) must return Handle\n"; exit; }
oxphp_async_await($releaser);
$pool->release($got);

echo "OK\n";
