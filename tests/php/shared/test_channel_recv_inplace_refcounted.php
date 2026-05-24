<?php
/**
 * Regression: Channel::recv() on the fiber WAKER path wraps refcounted
 * payloads correctly via the in-place helper.
 *
 * When recv() runs in the worker request fiber and the channel is empty, it
 * parks on a recv-waiter; a cross-thread send() delivers the payload and
 * oxphp_bridge_fiber_await returns 0 with a *materialized* zval in retval.
 * That branch wraps the value into RecvResult::Ok via
 * oxphp_bridge_wrap_result_ok_inplace, which ZVAL_COPYs the live payload
 * straight into the property instead of a portbuf serialize/deserialize
 * round-trip. This is the ONLY path that exercises the in-place wrap — the
 * thread-blocking and buffered-hit paths (covered elsewhere) go through
 * write_recv_ok with a pre-serialised buffer.
 *
 * The helper claims refcount soundness for scalars, strings, arrays and
 * objects (including Shared\* handles / tag-7). A premature free would
 * surface as a wrong value or UAF crash; a leak would not fail this test
 * (needs leak instrumentation) but a corrupted value does. We verify a
 * string, an array, and a Shared\Counter handle round-trip with the right
 * contents.
 *
 * Topology: consumer = this request fiber (recv() forever — no deadline, so
 * no spurious-timeout interaction); producer = oxphp_async pool closure that
 * paces its sends so the consumer parks on an empty channel first, forcing
 * the recv-waiter (in-place) path rather than a buffered hit. Runs only
 * under worker mode (needs the scheduler fiber).
 */
header('Content-Type: text/plain');

if (!oxphp_is_worker()) {
    echo "OK skip: not worker mode\n";
    return;
}

$ch = new OxPHP\Shared\Channel(1);

$producer = oxphp_async(function () use ($ch): void {
    // Pace so the request-fiber consumer parks on an empty channel before
    // each send → the send hands the payload to the parked recv-waiter →
    // fiber_await returns 0 → the in-place wrap runs. The Shared\Counter is
    // created and sent without an outer keepalive: the channel now holds it
    // alive in transit (the in-transit lifetime gap is fixed), so this also
    // exercises that the delivered handle is live.
    usleep(10_000);
    $ch->send('hello-string');
    usleep(10_000);
    $ch->send([1, 2, 3, 'k' => 'v']);
    usleep(10_000);
    $counter = new OxPHP\Shared\Counter();
    $counter->add(42);
    $ch->send($counter);
    $ch->close();
});

$r1 = $ch->recv(); // string
$r2 = $ch->recv(); // array
$r3 = $ch->recv(); // Shared\Counter handle

oxphp_async_await($producer);

if (!$r1->isOk() || $r1->value() !== 'hello-string') {
    echo 'FAIL: string payload via in-place wrap: ', var_export($r1->value(), true), "\n";
    return;
}
if (!$r2->isOk() || $r2->value() !== [1, 2, 3, 'k' => 'v']) {
    echo 'FAIL: array payload via in-place wrap: ', var_export($r2->value(), true), "\n";
    return;
}
if (!$r3->isOk()) {
    echo "FAIL: Shared\\Counter payload not Ok\n";
    return;
}
$counter = $r3->value();
if (!($counter instanceof OxPHP\Shared\Counter) || $counter->get() !== 42) {
    echo 'FAIL: Shared\\Counter via in-place wrap: ', var_export($counter, true), "\n";
    return;
}

echo "OK\n";
