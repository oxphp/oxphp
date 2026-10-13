<?php

declare(strict_types=1);

// Opens a session, writes to it and closes it, then parks for a second and
// reports what it finds of its own session when it wakes.
//
// session_write_close() ends the session but leaves its id, its array and the
// $_SESSION entry where they were, and the next session_start() on the thread
// takes that id before it looks at any cookie. So what a request admitted in
// this window must not find is all of that; and what this one must find when it
// wakes is all of it, because closing a session is not the same as giving it up.

set_error_handler(null);
set_exception_handler(null);

// PHP 8.6 turns session.use_strict_mode on by default, and strict mode swaps an
// id the store has never seen for a fresh one. The id here is the client's
// cookie, and the test reads it back, so it has to be kept.
ini_set('session.use_strict_mode', '0');

session_start();
$_SESSION['who'] = 'the request that closed it';
session_write_close();
$closedAt = microtime(true);

oxphp_sleep(1.0);

$doneAt = microtime(true);

header('Content-Type: application/json');
echo json_encode([
    'closed_at' => $closedAt,
    'done_at' => $doneAt,
    'id' => session_id(),
    'session_isset' => isset($_SESSION),
    'who' => $_SESSION['who'] ?? 'none',
    'status' => session_status(),
]);
