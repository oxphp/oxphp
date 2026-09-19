<?php

declare(strict_types=1);

// The other half of what a request can leave behind: this one does call
// session_write_close(), which is the discipline an application is told to
// follow. That clears the session's active status and nothing else —
// php_session_flush() writes the data and marks the session none, leaving the id
// where it is — so the state this request leaves is a closed session whose id is
// still installed on the thread.
//
// It is a separate fixture from fixture_session_leave_open.php because the two
// reach a request through different upstream paths: the open one is refused by
// session_start()'s already-active branch, the closed one is adopted by the
// branch that only consults the cookie when no id is installed.

// Cleared for the reason fixture_session_leave_open.php gives: worker-mode
// handlers outlive the request that installed them, and on a build that leaks
// the session this fixture's own session_start() raises the already-active
// notice — which a handler that throws would turn into an empty response, hiding
// the leak this fixture exists to set up.
set_error_handler(null);
set_exception_handler(null);

session_start();
$_SESSION['who'] = 'A2';
session_write_close();

header('Content-Type: text/plain');
echo 'SEEDED-CLOSED:' . session_id();
