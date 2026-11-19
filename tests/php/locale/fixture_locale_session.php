<?php

declare(strict_types=1);

// Driven over HTTP by locale/test_setlocale_reset_before_session_shutdown_write.
//
//   token=<hex>  name of the record the save handler leaves in the temp dir
//
// Switches LC_CTYPE to C and leaves its session open, so that the session is
// written from the session module's own shutdown rather than by this script.
// The save handler records the LC_CTYPE it writes under, then switches LC_CTYPE
// to C itself.
header('Content-Type: application/json');

$token = (string) ($_GET['token'] ?? '');
if (preg_match('/^[0-9a-f]{16}$/', $token) !== 1) {
    http_response_code(400);
    exit;
}

final class LocaleRecordingSessionHandler implements SessionHandlerInterface
{
    public function __construct(private readonly string $record)
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
        return '';
    }

    public function write(string $id, string $data): bool
    {
        file_put_contents($this->record, json_encode(['ctype' => setlocale(LC_CTYPE, '0')]));
        setlocale(LC_CTYPE, 'C');

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
}

// false: no shutdown function writes the session before the modules shut down.
session_set_save_handler(
    new LocaleRecordingSessionHandler(sys_get_temp_dir() . "/oxphp-locale-session-$token.json"),
    false
);
session_start(['use_cookies' => false]);
$_SESSION['seen'] = true;

echo json_encode([
    'worker' => oxphp_worker_id(),
    'set' => setlocale(LC_CTYPE, 'C'),
]);
