<?php
/**
 * Regression (in-transit lifetime): a Shared\* sent into a channel stays
 * alive in transit on the WAKER path. The consumer parks on an empty channel
 * first; the producer then creates the value, sends it fire-and-forget, and
 * returns (dropping its handle). The value is handed straight to the parked
 * recv-waiter; its keepalive must ride the AsyncResult so the entry survives
 * until the fiber materializes it.
 */
header('Content-Type: text/plain');

if (!oxphp_is_worker()) {
    echo "OK skip: not worker mode\n";
    return;
}

$ch = new OxPHP\Shared\Channel(1);

$producer = oxphp_async(function () use ($ch): void {
    usleep(20_000); // let the consumer park on the empty channel first
    $c = new OxPHP\Shared\Counter();
    $c->add(7);
    $ch->send($c); // hands straight to the parked recv-waiter
    // $c dropped here — only the in-transit keepalive keeps it alive.
});

$r = $ch->recv(); // parks, then wakes with the delivered value
oxphp_async_await($producer);

if (!$r->isOk()) {
    echo "FAIL: recv not ok\n";
    return;
}
$c = $r->value();
if (!($c instanceof OxPHP\Shared\Counter) || $c->get() !== 7) {
    echo 'FAIL: counter via waker transit: ', var_export($c, true), "\n";
    return;
}

echo "OK\n";
