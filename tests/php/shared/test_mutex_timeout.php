<?php
header('Content-Type: text/plain');

$m = new OxPHP\Shared\Mutex(0);

// Holder sleeps 3 seconds.
$held = oxphp_async(function() use ($m) {
    $m->withLockTimeout(function(&$s) {
        sleep(3);
    }, 10000);
});
usleep(200000); // 200ms — let the async acquire

$caught = false;
try {
    $m->withLockTimeout(fn(&$s) => 1, 500);
} catch (OxPHP\Shared\OperationTimeoutException $e) {
    $caught = true;
}
if (!$caught) { echo "FAIL: expected OperationTimeoutException\n"; exit; }

oxphp_async_await($held);

echo "OK\n";
