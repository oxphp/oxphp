<?php

declare(strict_types=1);

// Inner self-request for fibers/test_last_error_survives_suspend.
//
// Reports the last error it can see at its own start — which must be none, since
// it has raised none — and then raises one of its own, which the parked request
// must not come back reading.
//
// Must not suspend: this request has to run and finish inside the window the
// outer one is parked for.

// The handlers a worker runs with are whatever the last request to set them
// installed. Cleared so trigger_error below records a last error rather than
// being turned into an exception by somebody else's handler.
set_error_handler(null);
set_exception_handler(null);

$seen = error_get_last();

header('Content-Type: text/plain');
echo 'INNER-LAST-ERROR-AT-START:', $seen === null ? 'none' : $seen['message'], "\n";

// Silenced so the response carries only the markers the test reads; @ stops the
// display, not the record, which is the whole point of the check.
@trigger_error('inner request error', E_USER_WARNING);

echo "INNER-ERROR-RAISED\n";
