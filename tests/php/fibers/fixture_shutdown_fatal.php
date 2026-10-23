<?php

declare(strict_types=1);

// Inner self-request for fibers/test_gc_survives_shutdown_fatal.
//
// The fatal is raised from a shutdown function, which is the one place a worker
// runs PHP outside the protection that catches a request's own fatal: the
// engine's shutdown-function call is guarded by a try of its own with nothing
// behind it, so the bailout stops there and the worker is handed what looks like
// a request that ended normally — while everything the fatal left behind is
// still in place.
//
// Must not suspend: this request has to run and finish inside the window the
// outer one is parked for.

// The error and exception handlers a worker runs with are whatever the last
// request to set them installed — the outer test's, here, which turn errors into
// exceptions and report them. The fatal below is handed to neither. Cleared
// first all the same, so that nothing else this request raises can end it in an
// exception reported through somebody else's handler instead of in the fatal
// the test is about.
set_error_handler(null);
set_exception_handler(null);

require_once __DIR__ . '/../engine_fatal.php';

register_shutdown_function(static function (): void {
    engine_fatal('OxphpFatalFromAShutdownFunction');
});

header('Content-Type: text/plain');
echo "SHUTDOWN-FATAL-ARMED\n";
