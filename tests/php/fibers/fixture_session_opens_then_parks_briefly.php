<?php

declare(strict_types=1);

// Opens a session and parks, briefly — the request that will finish FIRST while
// the request admitted beside it is still going.
//
// Its job is to be the one that opened the session and then to get out of the
// way. What the worker does after it leaves, while its neighbour is still
// working in that session, is the thing under test.

set_error_handler(null);
set_exception_handler(null);

// PHP 8.6 turns session.use_strict_mode on by default, and strict mode swaps an
// id the store has never seen for a fresh one. The id here is the client's
// cookie, and the test reads it back, so it has to be kept.
ini_set('session.use_strict_mode', '0');

session_start();
$_SESSION['who'] = 'the request that opened it';

// Short: this must return while the neighbour admitted behind it is still
// parked, so the worker reaches an admission with the opener gone and the
// neighbour alive.
oxphp_sleep(0.3);

header('Content-Type: text/plain');
echo 'OPENER-DONE:' . session_id();
