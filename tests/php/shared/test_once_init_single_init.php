<?php
// getOrInit under N concurrent workers: factory runs exactly once.
header('Content-Type: text/plain');

$o = new OxPHP\Shared\Once();
$counter = new OxPHP\Shared\Counter();

$promises = [];
for ($i = 0; $i < 10; $i++) {
    $promises[] = oxphp_async(function() use ($o, $counter) {
        $v = $o->getOrInit(function() use ($counter) {
            $counter->add();
            usleep(5000);
            return 'singleton-value';
        });
        return $v;
    });
}
$results = oxphp_async_await_all($promises);

foreach ($results as $idx => $r) {
    if ($r !== 'singleton-value') {
        echo "FAIL: worker $idx got " . var_export($r, true) . "\n"; exit;
    }
}
if ($counter->get() !== 1) {
    echo "FAIL: factory ran " . $counter->get() . " times (want 1)\n"; exit;
}

echo "OK\n";
