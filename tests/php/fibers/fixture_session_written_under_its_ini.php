<?php

declare(strict_types=1);

// Inner request for fibers/test_session_is_written_under_its_requests_ini.
//
// Opens a session through a handler that records what it is asked to write,
// puts a float in it under a serialize_precision of its own, and ends without
// closing the session — leaving the write to the server, as most applications
// do. Must not suspend.

require_once __DIR__ . '/session_write_probe.php';

session_set_save_handler(new class implements SessionHandlerInterface {
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
        OxphpSessionWriteProbe::$written = $data;
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
}, false);

ini_set('session.use_cookies', '0');
session_id('oxphpsessionini');
session_start();

ini_set('serialize_precision', '5');
$_SESSION['sum'] = 0.1 + 0.2;

echo 'left open';
