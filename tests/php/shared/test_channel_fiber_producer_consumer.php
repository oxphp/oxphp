<?php
/**
 * Channel — fiber-mode producer/consumer under worker mode.
 *
 * Verifies that Channel::recv / Channel::send inside a fiber suspend
 * cooperatively via oxphp_bridge_fiber_await rather than blocking the
 * underlying OS worker thread. Both producer and consumer run as fibers
 * dispatched through oxphp_async(); the consumer recv's items until the
 * channel is closed, summing them.
 *
 * Sum assertion: 0+1+...+9 = 45.
 *
 * This test only runs under worker mode. In traditional mode there is no
 * persistent fiber scheduler available to the request, so we skip with a
 * visible marker in the output.
 */
header('Content-Type: text/plain');

if (!oxphp_is_worker()) {
    echo "OK skip: not worker mode\n";
    return;
}

$ch = new OxPHP\Shared\Channel(4);
$expected = 45; // sum 0..9

// Producer: send 10 items, 2ms apart; close when done.
$producer = oxphp_async(function () use ($ch) {
    for ($i = 0; $i < 10; $i++) {
        $ch->sendTimeout($i, 5000);
        usleep(2_000);
    }
    $ch->close();
});

// Consumer: recv until closed+drained (recv returns RecvResult::Closed), summing.
$consumer = oxphp_async(function () use ($ch) {
    $sum = 0;
    while (true) {
        $r = $ch->recvTimeout(2000);
        if (!$r->isOk()) {
            break;
        }
        $sum += $r->value();
    }
    return $sum;
});

// Await producer first (ensures close was called), then consumer for its result.
oxphp_async_await($producer);
$sum = oxphp_async_await($consumer);

if ($sum !== $expected) {
    echo "FAIL: expected sum $expected, got " . var_export($sum, true) . "\n";
    exit;
}

echo "OK\n";
