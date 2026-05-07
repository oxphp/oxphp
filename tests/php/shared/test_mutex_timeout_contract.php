<?php
/**
 * Mutex — unified timeout contract on with():
 *   null=forever, 0.0=try, INF=forever, NaN→Type, negative→Type.
 */
header('Content-Type: text/plain');

$m = new OxPHP\Shared\Mutex(0);

// 0.0 = try → with on contended mutex throws TimeoutException without blocking.
$holder = oxphp_async(function () use ($m) {
    $m->with(function (&$s) { sleep(2); }, null);
});
usleep(100_000);

$start = microtime(true);
$threw = false;
try {
    $m->with(fn(&$s) => 1, 0.0);
} catch (OxPHP\Shared\TimeoutException $e) {
    $threw = true;
}
$elapsed = microtime(true) - $start;
if (!$threw) { echo "FAIL: with(0.0) on contended must throw TimeoutException\n"; exit; }
if ($elapsed >= 0.05) { echo "FAIL: with(0.0) must be immediate, elapsed=$elapsed\n"; exit; }

oxphp_async_await($holder);

// NaN, negative → TypeException.
foreach ([NAN, -0.5] as $bad) {
    $caught = null;
    try {
        $m->with(fn(&$s) => 1, $bad);
    } catch (OxPHP\Shared\TypeException $e) {
        $caught = $e;
    }
    if ($caught === null) { echo "FAIL: with(" . var_export($bad, true) . ") must throw TypeException\n"; exit; }
}

// null = forever — short holder, ensure with() returns the body's value.
$m2 = new OxPHP\Shared\Mutex(0);
$short = oxphp_async(function () use ($m2) {
    $m2->with(function (&$s) { usleep(50_000); $s = 7; }, null);
});
$result = $m2->with(fn(&$s) => $s + 1, null);
if ($result !== 8) { echo "FAIL: with(null) wait-then-read got " . var_export($result, true) . "\n"; exit; }
oxphp_async_await($short);

echo "OK\n";
