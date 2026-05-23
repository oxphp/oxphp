<?php
// N async workers CAS-loop increment one key; no lost updates (linearisable).
header('Content-Type: text/plain');
$m = new OxPHP\Shared\Map();
$m->set('n', 0);

$promises = [];
for ($w = 0; $w < 8; $w++) {
    $promises[] = oxphp_async(function () use ($m) {
        for ($i = 0; $i < 100; $i++) {
            do {
                $cur = $m->get('n');
            } while (!$m->compareAndSet('n', $cur, $cur + 1));
        }
        return true;
    });
}
oxphp_async_await_all($promises);

if ($m->get('n') !== 800) {
    echo "FAIL: lost updates, final = " . $m->get('n') . " (want 800)\n";
    exit;
}
echo "OK\n";
