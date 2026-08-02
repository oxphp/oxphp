<?php

declare(strict_types=1);

// Inner self-request for fibers/test_shutdown_throw_is_reported.
//
// The exception is thrown from a shutdown function, which every other SAPI calls
// with no frame on the stack — that is what turns an exception thrown there into
// the "Uncaught ..." fatal it is everywhere else. A worker calls them from inside
// the frame its request loop runs in, so nothing in the engine reports this one.
//
// Must not suspend: this request has to run and finish inside the window the
// outer one is parked for.

// The error and exception handlers a worker runs with are whatever the last
// request to set them installed — the outer test's, here. Cleared first, so this
// exception reaches the end of the request the way an uncaught one does rather
// than being caught and printed by somebody else's handler.
set_error_handler(null);
set_exception_handler(null);

register_shutdown_function(static function (): void {
    throw new \RuntimeException('thrown from a shutdown function');
});

header('Content-Type: text/plain');
echo "SHUTDOWN-THROW-ARMED\n";
