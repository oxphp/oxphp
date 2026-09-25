<?php

declare(strict_types=1);

// Inner request for fibers/test_session_write_keeps_the_worker.
//
// Opens a session through a handler whose write sleeps, and ends without
// closing it, so the write happens as the server ends the request. The sleep is
// hooked in this profile: a request that is allowed to park there hands the
// worker to the next request while its session is still on the thread.

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
        OxphpSessionWriteProbe::$writing = true;
        usleep(300_000);
        OxphpSessionWriteProbe::$writing = false;
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

    // PHP 8.6 warns once for each of these a handler class lacks, and a warning
    // here ends the request, or its headers, before its session has started.
    public function create_sid(): string
    {
        return bin2hex(random_bytes(16));
    }

    public function validateId(string $id): bool
    {
        return true;
    }
}, false);

ini_set('session.use_cookies', '0');
session_id('oxphpsessionwriteparks');
session_start();
$_SESSION['who'] = 'writer';

echo 'left open';
