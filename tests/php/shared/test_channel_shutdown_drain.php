<?php
/**
 * Channel — simulate shutdown drain via explicit close() from another fiber.
 *
 * We can't trigger real SIGTERM shutdown from within a single test, but the
 * drain path is exactly `on_shutdown_notify() -> close()`. Closing the
 * channel from another fiber/async exercises the same wake path.
 */
header('Content-Type: text/plain');

if (!oxphp_is_worker()) {
    echo "OK skip: not worker mode\n";
    return;
}

$ch = new OxPHP\Shared\Channel(1);
$ch->send('pre-close'); // fill so future send would block

// Consumer: will block on recv; we expect null after the delayed close.
$consumer = oxphp_async(function () use ($ch) {
    $ch->recv(); // drain 'pre-close'
    $second = $ch->recv(); // blocks; close should wake it with null
    return $second;
});

// Simulate drain: from another fiber, after a short delay, close.
$closer = oxphp_async(function () use ($ch) {
    usleep(50_000); // 50ms
    $ch->close();
    return true;
});

oxphp_async_await($closer);
$result = oxphp_async_await($consumer);

if ($result !== null) {
    echo "FAIL: blocked recv after close should return null, got " . var_export($result, true) . "\n";
    exit;
}

echo "OK\n";
