<?php
header('Content-Type: text/plain');

$m = new OxPHP\Shared\Mutex(0);

// Holder sleeps 3 seconds.
$held = oxphp_async(function() use ($m) {
    $m->with(function(&$s) {
        sleep(3);
    }, 10.0);
});
usleep(200000); // 200ms — let the async acquire

$caught = false;
try {
    $m->with(fn(&$s) => 1, 0.5);
} catch (OxPHP\Shared\TimeoutException $e) {
    $caught = true;
}
if (!$caught) { echo "FAIL: expected TimeoutException\n"; exit; }

oxphp_async_await($held);

echo "OK\n";
