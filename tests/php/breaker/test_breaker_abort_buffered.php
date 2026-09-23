<?php

declare(strict_types=1);

// A request that finishes, whose client is not there to receive it.
//
// Everything this handler produces goes into an output buffer of its own, so
// nothing is written while it runs: the disconnect is recorded by the interrupt
// handler and not acted on; the echo lands in the buffer; the handler returns.
// The request is over at that point, and what is left is the server's own work
// — the flush that empties the buffer onto a connection nobody is holding any
// more.
//
// That flush meets the cancellation check on the write path, and lets it pass:
// the request is not streaming, so a client that has left does not end it at a
// write (ignore_user_abort(true) below would let it pass all the same). The
// output goes nowhere and the request is done. A request that ran to its end is
// a request the worker served, and it has to clear the consecutive-error run
// like any other — the client leaving before the last byte says something about
// the client, not about the worker.
//
// The suite runs this between fatals for that reason. Two fatals, this, then a
// third fatal: if the run is cleared here the third fatal is the first of a new
// run and the worker keeps serving, and if it is not, three failures stand in a
// row and the worker is retired. The probe that follows reads which happened.

$marker = '/tmp/oxphp-breaker-abort-buffered';
// is_file first, and no @, for the reason the abort fixture beside this one
// gives: an error handler installed by an earlier probe in this suite is still
// registered here and would turn the warning into an uncaught exception.
if (is_file($marker)) {
    unlink($marker);
}

ob_start();

ignore_user_abort(true);

// Longer than the suite line's --max-time, so the client is gone by the time
// the handler returns. Native blocking sleep: no runtime hooks in this profile.
usleep(2_000_000);

echo "buffered; nobody is left to read this\n";

// Written before the buffer is flushed — the handler reached its own end, which
// is the premise of this case and not something the probe could otherwise tell
// from a request that died halfway.
@file_put_contents($marker, 'reached');
