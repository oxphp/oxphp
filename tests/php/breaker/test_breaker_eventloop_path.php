<?php

declare(strict_types=1);

// The other dispatch site. Everything above this point runs on the serve loop's
// fast path — the branch taken when the worker has no live fiber to come back
// to, which is the branch the breaker used to be blind on. Requests dispatched
// from the event loop's tick reach the same exit check, and that is what this
// covers.
//
// The shape: three inner requests are queued and then this request parks in
// oxphp_sleep(), which is what puts the worker into its event-loop branch —
// there is now a fiber to come back to, so the loop ticks instead of
// block-waiting, and the tick is what accepts and runs the three. Each of them
// fatals, the third takes the count to the threshold, and the exit check at the
// bottom of that same iteration retires the worker.
//
// This request is the casualty and that is the assertion: it is still parked
// when the worker leaves its loop, so it is ended the way a retiring worker ends
// the requests it was multiplexing — cancelled at its suspend point and answered
// 503. A build where the tick's fatals never reach the breaker leaves it to wake
// up and answer 200 instead, which is the suite line's 503 going red.
//
// oxphp_sleep() rather than sleep(): this profile runs no hooks, so a native
// sleep would block the worker and never yield the event loop.
//
// No TestCase: this request does not get to report anything.

$socks = [];
for ($i = 1; $i <= 3; $i++) {
    $sock = @stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        // Nothing else to do with it: the response this request would carry the
        // failure in is one it will not live to send.
        continue;
    }
    fwrite($sock, "GET /tests/breaker/test_breaker_fatal.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $socks[] = $sock;
}

// Long enough for the tick to accept and run all three, short enough that a
// build which never retires the worker ends this request well inside the
// runner's 15 s rather than holding the profile's single worker past it.
oxphp_sleep(4.0);

foreach ($socks as $sock) {
    @fclose($sock);
}

echo "unreachable: three fatals from the tick must retire this worker\n";
