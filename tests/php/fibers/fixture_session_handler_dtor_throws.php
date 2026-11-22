<?php

declare(strict_types=1);

// Roles around a session held open across a park whose save handler throws out
// of its destructor, for test_exception_between_slices_stays_out_of_requests.
//
// ?role=holder registers a userland save handler that nothing but the session
// module holds, opens its session and parks with it open for a second. The
// neighbour registers a handler of its own in that second, so once the holder's
// session is written and closed nothing holds the holder's handler any more,
// and its destructor runs between two slices of the worker, in no request.
// What that destructor throws is an exception whose own destructor throws, so
// one discard of it leaves another standing.
//
// ?variant=resume: the neighbour then opens its own session, which waits for
// the holder's to close, so the next thing the worker does after the holder is
// resume the neighbour.
//
// ?variant=admission: the neighbour opens nothing and ends, and the holder,
// just before it ends, sends one more request (?role=late) to this worker, so
// the next thing the worker does after the holder is take that new request. It
// writes ?marker= when its handler runs. The connection is kept in the worker's
// shared state rather than in a local: one the holder's end closed would be a
// client that left before its request was taken.

set_error_handler(null);
set_exception_handler(null);

$role = (string) ($_GET['role'] ?? '');
$variant = (string) ($_GET['variant'] ?? '');
$marker = (string) ($_GET['marker'] ?? '');

if (!class_exists('OxphpSessionDtorThrowsHandler', false)) {
    final class OxphpDroppedExceptionWithThrowingDestructor extends RuntimeException
    {
        public function __destruct()
        {
            throw new LogicException('thrown by the destructor of the exception the handler threw');
        }
    }

    class OxphpSessionDtorQuietHandler implements SessionHandlerInterface
    {
        public function open(string $path, string $name): bool
        {
            return true;
        }

        public function close(): bool
        {
            return true;
        }

        public function read(string $id): string
        {
            return '';
        }

        public function write(string $id, string $data): bool
        {
            return true;
        }

        public function destroy(string $id): bool
        {
            return true;
        }

        public function gc(int $max_lifetime): int|false
        {
            return 0;
        }

        // PHP 8.6 warns once for each of these a handler class lacks.
        public function create_sid(): string
        {
            return bin2hex(random_bytes(16));
        }

        public function validateId(string $id): bool
        {
            return true;
        }
    }

    final class OxphpSessionDtorThrowsHandler extends OxphpSessionDtorQuietHandler
    {
        // The variant goes into the message so that the server log line the
        // dropped exception gets tells the two places it was dropped apart.
        public function __construct(private readonly string $variant)
        {
        }

        public function __destruct()
        {
            throw new OxphpDroppedExceptionWithThrowingDestructor(
                'thrown by the holder\'s save handler destructor [' . $this->variant . ']'
            );
        }
    }
}

// PHP 8.6 turns session.use_strict_mode on by default; the ids here are the
// clients' cookies.
ini_set('session.use_strict_mode', '0');

header('Content-Type: application/json');

if ($role === 'holder') {
    session_set_save_handler(new OxphpSessionDtorThrowsHandler($variant), false);
    session_start();
    $_SESSION['who'] = 'holder';
    oxphp_sleep(1.0);

    if ($variant === 'admission') {
        $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
        if ($sock !== false) {
            fwrite($sock, 'GET /tests/fibers/fixture_session_handler_dtor_throws.php?role=late&marker='
                . rawurlencode($marker) . " HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
            $sharedState['session_dtor_throws_late'] = $sock;
        }
    }

    session_write_close();
    echo json_encode(['who' => 'holder', 'sent_late' => isset($sharedState['session_dtor_throws_late'])]);
    return;
}

if ($role === 'neighbour') {
    $sharedState['session_dtor_throws_parked'] = true;
    oxphp_sleep(0.4);

    $configured = session_set_save_handler(new OxphpSessionDtorQuietHandler(), false);
    if ($variant === 'resume') {
        session_start();
        $_SESSION['who'] = 'neighbour';
        session_write_close();
    }

    echo json_encode(['who' => 'neighbour', 'configured' => $configured]);
    return;
}

if ($role === 'late') {
    file_put_contents($marker, 'ran');
    echo json_encode(['who' => 'late']);
    return;
}

http_response_code(400);
echo "unknown role: $role";
