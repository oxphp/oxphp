<?php

declare(strict_types=1);

// A request whose client hangs up on a handler that asked to outlive it.
//
// In worker mode a request that is not streaming is not ended by its client
// leaving whether or not it calls ignore_user_abort(true), so this runs the
// same way as the abort earlier in the suite: the interrupt handler records the
// disconnect and returns, the first write after that goes through the SAPI's
// output path, which reads the same cancel cell and lets the request go on too,
// and the handler runs to its own end with nobody reading what it writes. It
// completes, and the breaker reads it so: counting it would mean a client
// hanging up three times retires a worker.
//
// The markers are what make this test mean anything. Nobody is left to read the
// response, so the suite line can only assert that curl gave up — which it does
// whether or not the server ever noticed, and would go on doing if cancellation
// stopped reaching PHP entirely. All three are cleared on the way in, so what a
// probe reads is about the last request that got here and not about the first:
//
//  - the shutdown marker says this request reached the end of its request, and
//    carries connection_status() for diagnosis only;
//  - the past-echo marker must be PRESENT. It is written after the echo, so it
//    is reached only if that write did not end the request — which in worker
//    mode is the whole point: the handler gets to finish and run its own
//    cleanup. A run where it is missing is a run in which the write ended the
//    request where it stood;
//  - the past-sleep marker says the interrupt handler let the request go on. It
//    sits between the sleep and the echo, so a request unwound on the client's
//    departure stops at the first opcode after the sleep and never writes it.
//    Only the probe that follows a single abort asserts it, for the reason
//    given beside that line in the suite.
//
// Files rather than statics: the retires this suite performs replace the
// worker, and worker-scope state does not survive that. /tmp is inside the
// container and starts empty with it.

// is_file first, and no @: a TestCase built by a probe earlier in this suite
// installs an error handler that turns a warning into an ErrorException, and it
// is still installed here — set_error_handler outlives the request that called
// it on a worker that keeps serving. The silence operator does not stop a
// registered handler from running either, so unlinking a marker that is not
// there would end this request as an uncaught exception rather than as the
// abort it is about.
foreach ([
    '/tmp/oxphp-breaker-abort-ignored',
    '/tmp/oxphp-breaker-abort-ignored-past-sleep',
    '/tmp/oxphp-breaker-abort-ignored-past-echo',
] as $marker) {
    if (is_file($marker)) {
        unlink($marker);
    }
}

register_shutdown_function(static function (): void {
    @file_put_contents('/tmp/oxphp-breaker-abort-ignored', (string) connection_status());
});

ignore_user_abort(true);

// usleep rather than a busy loop: with no runtime hooks in this profile it is a
// native blocking sleep, which keeps the request on the worker's fast path.
// Longer than the suite line's --max-time, so the client is gone on the way out.
usleep(2_000_000);

// Past the sleep with the request still running: the interrupt handler saw the
// client go and let the request go on instead of unwinding here.
@file_put_contents('/tmp/oxphp-breaker-abort-ignored-past-sleep', 'reached');

echo "the client is gone; nobody reads this write\n";

@file_put_contents('/tmp/oxphp-breaker-abort-ignored-past-echo', 'reached');
