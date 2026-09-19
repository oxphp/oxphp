<?php

declare(strict_types=1);

// Admitted while the opener is parked, so it is handed the opener's session —
// the documented overlap. Then it parks for long enough that the worker takes
// another request after the opener has finished, and reports whether the
// session it was working in was still there when it came back.
//
// It starts nothing of its own: on this path session_start() would answer the
// already-active session, which is the same array, and reporting on state it
// asked for rather than state it was handed would make the answer its own.

set_error_handler(null);
set_exception_handler(null);

$before = [
    'session_isset' => isset($_SESSION),
    'session_id' => session_id(),
    'who' => $_SESSION['who'] ?? 'none',
];

// Long enough for the opener to finish and for a third request to be taken by
// this worker in the window that leaves.
oxphp_sleep(1.5);

$after = [
    'session_isset' => isset($_SESSION),
    'session_id' => session_id(),
    'who' => $_SESSION['who'] ?? 'none',
];

header('Content-Type: application/json');
echo json_encode(['before' => $before, 'after' => $after]);
