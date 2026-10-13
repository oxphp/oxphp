<?php

declare(strict_types=1);

// Starts a session through a save handler whose read() parks for a while, the
// way a handler reading the store over a hooked database or cache connection
// does, and reports when the read began and ended and when the request was done
// with the session.
//
// The session counts as open from before the handler is first called: the
// module marks it active, then opens the handler and reads. So the whole read
// is inside the window in which no other request may run on this worker.

set_error_handler(null);
set_exception_handler(null);

// PHP 8.6 turns session.use_strict_mode on by default, and strict mode swaps an
// id the store has never seen for a fresh one. The id here is the client's
// cookie, and the test reads it back, so it has to be kept.
ini_set('session.use_strict_mode', '0');

$handler = new class ((string) ($_COOKIE['PHPSESSID'] ?? '')) implements SessionHandlerInterface {
    public float $readEnteredAt = 0.0;
    public float $readLeftAt = 0.0;

    // Only this request's own id parks. The handler stays registered on the
    // worker after this request, until the worker next resets with nothing in
    // flight, and the requests that use it meanwhile must not be slowed by it.
    public function __construct(private string $parkFor)
    {
    }

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
        if ($id === $this->parkFor) {
            $this->readEnteredAt = microtime(true);
            oxphp_sleep(0.6);
            $this->readLeftAt = microtime(true);
        }

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

    public function gc(int $max_lifetime): int
    {
        return 0;
    }

    // PHP 8.6 warns once for each of these a handler class lacks, and with the
    // error handler cleared above that warning would land in the body the test
    // parses.
    public function create_sid(): string
    {
        return bin2hex(random_bytes(16));
    }

    public function validateId(string $id): bool
    {
        return true;
    }
};
session_set_save_handler($handler, false);

session_start();
$_SESSION['who'] = 'the request whose handler parked';
$id = session_id();
session_write_close();
$doneAt = microtime(true);

header('Content-Type: application/json');
echo json_encode([
    'id' => $id,
    'read_entered_at' => $handler->readEnteredAt,
    'read_left_at' => $handler->readLeftAt,
    'done_at' => $doneAt,
]);
