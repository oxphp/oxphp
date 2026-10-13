<?php

declare(strict_types=1);

// A session whose save handler fails to close, for the test that what the
// failed close leaves behind does not outlive the request.
//
// ?role=close-throws opens the session through a SessionHandler subclass that
// hands storage to the files handler and whose close() throws before it lets
// the files handler close, then reports whether session_write_close() threw and
// what the session status was after it. The files handler is left open, with
// its lock on the session file: the id is the test's own and is never opened
// again.
//
// ?role=plain answers with the time it was served, and opens nothing.

set_error_handler(null);
set_exception_handler(null);

if (!class_exists('OxphpSessionCloseThrowsHandler', false)) {
    final class OxphpSessionCloseThrowsHandler extends SessionHandler
    {
        public function close(): bool
        {
            throw new RuntimeException('close refused');
        }
    }
}

header('Content-Type: application/json');

$role = (string) ($_GET['role'] ?? '');

if ($role === 'close-throws') {
    // PHP 8.6 turns session.use_strict_mode on by default; the id is the
    // client's cookie, and the test never wants a fresh one.
    ini_set('session.use_strict_mode', '0');
    // A SessionHandler hands storage to whichever handler was installed before
    // it, which an earlier request on this worker may have left as a userland
    // one: the files handler first, so that is the one it hands to.
    ini_set('session.save_handler', 'files');
    session_set_save_handler(new OxphpSessionCloseThrowsHandler(), false);

    session_start();
    $_SESSION['who'] = 'the request whose close throws';
    $id = session_id();
    $threw = false;
    try {
        session_write_close();
    } catch (RuntimeException) {
        $threw = true;
    }

    echo json_encode([
        'id' => $id,
        'threw' => $threw,
        'status_after' => session_status(),
    ]);
    return;
}

if ($role === 'plain') {
    echo json_encode(['served_at' => microtime(true)]);
    return;
}

http_response_code(400);
echo "unknown role: $role";
