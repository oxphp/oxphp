<?php

declare(strict_types=1);

// A request that will not take no for an answer: it catches the server's refusal
// to let it suspend itself and immediately tries again. Deliberately not a
// TestCase — it is the subject of test_suspend_retry_is_bounded, and it cannot
// report on its own ending.
//
// The refusal is issued from inside the scheduler tick, so an unbounded exchange
// here does not stall one request: the tick never returns and every other
// in-flight request on this worker stops with it. The server must therefore stop
// arguing at some point and end this request instead.
header('Content-Type: text/plain');

$attempts = 0;
while (true) {
    $attempts++;
    try {
        \Fiber::suspend();
    } catch (\FiberError $e) {
        // Exactly what a userland scheduler wrapping suspend() in a retry would
        // do. Keep going.
    }
}
