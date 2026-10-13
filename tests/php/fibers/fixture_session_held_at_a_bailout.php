<?php

declare(strict_types=1);

// The hundredth request of a fresh worker: opens a session under the client's
// cookie, writes into it, and parks with it open. The collector the worker runs
// once it has taken its hundredth request then runs the destructor
// fixture_session_bailout_cycle.php left behind, and the fatal error that
// destructor ends in ends the worker's serve loop while this request is parked
// holding the session — and with it the files handler's lock on the session.
// The probe then looks at what the worker script's own shutdown made of it.
//
// Driven by tests/scripts/verify_worker_session_closed_after_a_bailout.sh, not
// by a suite. Included inside the worker's handler, where $requestCount is in
// scope.

set_error_handler(null);
set_exception_handler(null);

// PHP 8.6 turns session.use_strict_mode on by default, and strict mode swaps an
// id the store has never seen for a fresh one. The probe looks the session up by
// the client's cookie, so it has to be kept.
ini_set('session.use_strict_mode', '0');

session_start();
$_SESSION['marker'] = 'written by the worker script shutdown';
file_put_contents('/tmp/oxphp-session-bailout-held', json_encode([
    'request_count' => $requestCount,
    'id' => session_id(),
]));

// Hooked: parks this request with its session open. The fatal error ends the
// worker long before the park would.
oxphp_sleep(5.0);

header('Content-Type: application/json');
echo json_encode(['reached' => 'the end of the park']);
