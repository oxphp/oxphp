<?php

declare(strict_types=1);

// A request the client left, whose shutdown function then fatals.
//
// The client goes first, so the request is on its way out as a cancellation
// before anything has failed: ignore_user_abort(true) keeps the interrupt
// handler from unwinding it, and in worker mode the echo below does not end it
// either, so it reaches the end of its own handler. Up to that point this is
// the fixture beside it, and up to that point the answer is the same — a client
// leaving is not a worker to replace.
//
// Then the shutdown function fatals. That is not the client's doing and it is
// not undone by the client having left first: the engine state it leaves is
// what the next request on this worker would inherit, exactly as it would from
// a fatal on a request nobody had abandoned. The order of the two events is the
// only thing separating this from the fixture beside it, and it has to be
// enough — otherwise a handler that fatals on every request keeps its worker for
// as long as clients keep hanging up, which under the abort storm this suite is
// about is indefinitely.
//
// Three of these must retire the worker.

$marker = '/tmp/oxphp-breaker-abort-then-shutdown-fatal';
// is_file first, and no @, for the reason the other abort fixtures give: an
// error handler installed by an earlier probe is still registered here.
if (is_file($marker)) {
    unlink($marker);
}

register_shutdown_function(static function () use ($marker): void {
    @file_put_contents($marker, 'reached');

    // A bare fatal raised from inside the shutdown window, by the mechanism
    // test_breaker_fatal picks and for the same reason: a class redeclaration
    // is E_COMPILE_ERROR, which cannot be routed to a user error handler, so
    // nothing an earlier request left installed can turn it into a throw.
    //
    // Deliberately not a call to an undefined function, which is what this
    // fixture used to do. That is an Error thrown, not a fatal raised, so it
    // leaves the request as an uncaught exception — an application outcome the
    // breaker does not count and must not, which test_breaker_throw pins. The
    // distinction is invisible in the log, where both arrive as E_ERROR.
    eval('class BreakerAbortThenShutdownDup {}');
    eval('class BreakerAbortThenShutdownDup {}');
});

ignore_user_abort(true);

// Longer than the suite line's --max-time, so the client is gone by the echo.
usleep(2_000_000);

echo "the client is gone; nobody reads this write\n";
