<?php
/**
 * Cross-thread test — Counter shared across ≥2 async workers.
 * Requires ASYNC_WORKERS >= 2 and serializer tag 7.
 */
header('Content-Type: text/plain');

$c = new OxPHP\Shared\Counter();
$n = 1000;
$promises = [];
for ($i = 0; $i < 4; $i++) {
    $promises[] = oxphp_async(function() use ($c, $n) {
        for ($j = 0; $j < $n; $j++) { $c->add(); }
    });
}
oxphp_async_await_all($promises);

$expected = 4 * $n;
$got = $c->get();
if ($got !== $expected) {
    echo "FAIL: expected $expected got $got\n"; exit;
}

echo "OK\n";
