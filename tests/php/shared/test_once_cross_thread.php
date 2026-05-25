<?php
// Once race — verify exactly-one-winner across async workers.
header('Content-Type: text/plain');

$once = new OxPHP\Shared\Once();
$winners = new OxPHP\Shared\Counter();

$promises = [];
for ($i = 0; $i < 4; $i++) {
    $n = $i + 1; // 1..4
    $promises[] = oxphp_async(function() use ($once, $winners, $n) {
        if ($once->trySet($n)) {
            $winners->add();
        }
    });
}
oxphp_async_await_all($promises);

if ($winners->get() !== 1) {
    echo "FAIL: expected exactly 1 winner, got " . $winners->get() . "\n"; exit;
}

$v = $once->get();
if (!is_int($v) || $v < 1 || $v > 4) {
    echo "FAIL: unexpected Once value " . var_export($v, true) . "\n"; exit;
}

echo "OK\n";
