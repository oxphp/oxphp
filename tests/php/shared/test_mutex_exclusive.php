<?php
header('Content-Type: text/plain');

$m = new OxPHP\Shared\Mutex(0);
$n = 100;
$promises = [];
for ($i = 0; $i < 4; $i++) {
    $promises[] = oxphp_async(function() use ($m, $n) {
        for ($j = 0; $j < $n; $j++) {
            $m->withLock(function(&$s) { $s++; });
        }
    });
}
oxphp_async_await_all($promises);

$got = $m->withLock(fn(&$s) => $s);
if ($got !== 4 * $n) {
    echo "FAIL: expected " . (4 * $n) . " got $got\n"; exit;
}

echo "OK\n";
