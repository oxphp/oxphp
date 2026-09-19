<?php

declare(strict_types=1);

// Reports the session state it finds and starts nothing of its own.
//
// The probe in fixture_session_probe.php answers what a request gets when it
// asks for a session; this one answers what is already standing before anything
// asks. That difference is the point here: the request this fixture runs beside
// is parked with a session open, so a fixture that called session_start() would
// be reading its own answer rather than the leftover.

// Cleared for the reason the other session fixtures give: handlers live for the
// life of the worker in worker mode, and one that turns a notice into a throw
// would end this request before it could report anything.
set_error_handler(null);
set_exception_handler(null);

header('Content-Type: application/json');
echo json_encode([
    'session_isset' => isset($_SESSION),
    'session_id' => session_id(),
    'who' => $_SESSION['who'] ?? 'none',
]);
