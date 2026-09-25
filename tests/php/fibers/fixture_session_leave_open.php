<?php

declare(strict_types=1);

// Seeds thread-wide session state and leaves it standing: this request starts a
// session and returns without session_write_close() and without
// session_destroy(), which is what an ordinary application does, because under
// PHP-FPM the end of the request writes and closes the session for it.
//
// A worker has no such end, so what this request leaves behind is exactly what
// the next request the worker takes finds in front of it. Served through the
// event loop — the outer test is parked on the read of this response — it is the
// state a tick-admitted request starts with.

// Cleared so this fixture answers whatever the session module does, rather than
// whatever the last request on this worker left installed: handlers live for the
// life of the worker in worker mode, and one that turns a notice into a throw
// would end this request before it could seed anything.
set_error_handler(null);
set_exception_handler(null);

// PHP 8.6 turns session.use_strict_mode on by default, and strict mode swaps an
// id the store has never seen for a fresh one. The id here is the client's
// cookie, and the test reads it back, so it has to be kept.
ini_set('session.use_strict_mode', '0');

session_start();
$_SESSION['who'] = 'A';

header('Content-Type: text/plain');
echo 'SEEDED-ACTIVE:' . session_id();
