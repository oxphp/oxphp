<?php

declare(strict_types=1);

// Reports what a request admitted through the event loop finds of the session
// its predecessor left, and what its own session_start() then does with its own
// cookie.
//
// Everything about the inherited state is read BEFORE this request starts a
// session of its own: once the start has run, a clean answer and a leaked one are
// no longer distinguishable from inside.
//
// The notice is the signature this probe exists for. session_start() answers TRUE
// on a session that is already active and never looks at the cookie or the
// store — so the only thing that separates "this request got its own session"
// from "this request was handed the previous one" at the moment it happens is the
// E_NOTICE the refusal raises.

$preSessionIsset = isset($_SESSION);
$preSessionId = session_id();

$notice = null;
// Installed over whatever handler the last request on this worker left: handlers
// persist for the life of the worker in worker mode, and one that converts
// notices to exceptions would end this request instead of reporting. Swallowed
// rather than displayed, so the notice cannot reach the body the test parses.
set_error_handler(static function (int $errno, string $message) use (&$notice): bool {
    $notice = $message;

    return true;
});
$started = session_start();
restore_error_handler();

$who = $_SESSION['who'] ?? 'none';

header('Content-Type: application/json');
echo json_encode([
    'pre_session_isset' => $preSessionIsset,
    'pre_session_id' => $preSessionId,
    'started' => $started,
    'notice' => $notice,
    'id' => session_id(),
    'who' => $who,
]);
