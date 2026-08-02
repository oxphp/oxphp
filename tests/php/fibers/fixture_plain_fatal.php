<?php

declare(strict_types=1);

// Inner self-request for fibers/test_gc_survives_fatal: a request whose only job
// is to take a fatal on the worker while another request is parked.
//
// The error and exception handlers a worker runs with are whatever the last
// request to set them installed — the outer test's, here, which turn errors into
// exceptions and report them. Cleared first, so what this request raises is the
// fatal the test is about rather than an exception reported through somebody
// else's handler.

set_error_handler(null);
set_exception_handler(null);

header('Content-Type: text/plain');
echo "PLAIN-FATAL-ARMED\n";

trigger_error('gc probe fatal', E_USER_ERROR);

echo "NOT-REACHED\n";
