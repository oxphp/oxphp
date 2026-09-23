<?php

declare(strict_types=1);

// Inner request for fibers/test_session_write_keeps_the_worker: queued behind a
// request whose session write sleeps. Reports the session it finds standing and
// where that write was when it ran. Starts nothing of its own.

require_once __DIR__ . '/session_write_probe.php';

// Cleared for the reason the other session fixtures give.
set_error_handler(null);
set_exception_handler(null);

header('Content-Type: application/json');
echo json_encode([
    'session_id' => session_id(),
    'who' => $_SESSION['who'] ?? 'none',
    'writing' => OxphpSessionWriteProbe::$writing,
    'written' => OxphpSessionWriteProbe::$written,
]);
