<?php
/**
 * Regression (in-transit lifetime): a Shared\* sent into a channel stays
 * alive in transit on the BUFFERED path even after the sender drops its
 * reference. The producer creates a Shared\Counter, sends it into a
 * capacity>=1 channel, and returns — dropping its local handle — BEFORE the
 * consumer recvs. Without the channel holding a strong ref in transit, the
 * entry would be freed and recv()->value() would return NULL.
 */
header('Content-Type: text/plain');

if (!oxphp_is_worker()) {
    echo "OK skip: not worker mode\n";
    return;
}

$ch = new OxPHP\Shared\Channel(4);

$producer = oxphp_async(function () use ($ch): void {
    $c = new OxPHP\Shared\Counter();
    $c->add(7);
    $ch->send($c); // buffered: no consumer parked yet
    // $c goes out of scope here — the producer's strong ref is dropped.
});
oxphp_async_await($producer); // send completed, producer ref dropped

$r = $ch->recv();
if (!$r->isOk()) {
    echo "FAIL: recv not ok\n";
    return;
}
$c = $r->value();
if (!($c instanceof OxPHP\Shared\Counter) || $c->get() !== 7) {
    echo 'FAIL: counter via buffered transit: ', var_export($c, true), "\n";
    return;
}

echo "OK\n";
