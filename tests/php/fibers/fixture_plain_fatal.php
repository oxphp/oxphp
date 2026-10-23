<?php

declare(strict_types=1);

// Inner self-request for fibers/test_gc_survives_fatal: a request whose only job
// is to take a fatal on the worker while another request is parked.
//
// The error and exception handlers a worker runs with are whatever the last
// request to set them installed — the outer test's, here, which turn errors into
// exceptions and report them. The fatal below is handed to neither. Cleared
// first all the same, so that nothing else this request raises can end it in an
// exception reported through somebody else's handler instead of in the fatal
// the test is about.

set_error_handler(null);
set_exception_handler(null);

require_once __DIR__ . '/../engine_fatal.php';

header('Content-Type: text/plain');
echo "PLAIN-FATAL-ARMED\n";

engine_fatal('OxphpGcProbeFatal');

echo "NOT-REACHED\n";
