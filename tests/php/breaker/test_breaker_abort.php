<?php

declare(strict_types=1);

// A request whose client hangs up while it is still running.
//
// The suite line gives curl a deadline shorter than the sleep below, so curl
// closes the connection first; the server notices, sets the cancel reason and
// raises the interrupt. This request never suspends, so the interrupt is taken
// at the first opcode after the sleep returns — and in worker mode, for a
// request that is not streaming, it records the disconnect and lets the request
// go on. So does the write after it. The request runs to its own end, and the
// breaker reads it as a request that completed.
//
// usleep rather than a busy loop: with no runtime hooks in this profile it is a
// native blocking sleep, which keeps the request on the worker's fast path.
//
// The marker is what makes this test mean anything. Nobody is left to read the
// response, so the suite line can only assert that curl gave up — which it does
// whether or not the server ever noticed, and would go on doing if cancellation
// stopped reaching PHP entirely. The shutdown function below runs on the
// cancelled request and records the connection state the interrupt handler set,
// and the probe that follows reads it: a request whose disconnect never reached
// PHP writes 0 (PHP_CONNECTION_NORMAL) and fails the probe. A second marker,
// after the write, says the request was not ended there either.
//
// A file rather than a static: the retire this suite performs earlier replaces
// the worker, and worker-scope state does not survive that. /tmp is inside the
// container and starts empty with it; both markers are still cleared on the way
// in, so that a rerun against a live container reads only this run's. is_file
// before unlink, not the silence operator: a registered error handler still runs
// under @, and would turn the warning for a missing file into an exception.
foreach (['/tmp/oxphp-breaker-abort', '/tmp/oxphp-breaker-abort-past-echo'] as $marker) {
    if (is_file($marker)) {
        unlink($marker);
    }
}

register_shutdown_function(static function (): void {
    @file_put_contents('/tmp/oxphp-breaker-abort', (string) connection_status());
});

usleep(2_000_000);

echo "the client is gone by now; nobody reads this write\n";

@file_put_contents('/tmp/oxphp-breaker-abort-past-echo', 'past-echo');
