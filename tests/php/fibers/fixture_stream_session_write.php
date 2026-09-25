<?php

declare(strict_types=1);

// Inner request for fibers/test_stream_session_write_after_client_left.
//
// A stream with an open session, whose save handler writes output as the
// session is written. It parks until its client has gone, then returns without
// writing anything itself, so the first write after its client left is the
// save handler's — made after the request has returned, while the worker gives
// its session back. A stream that has not asked to outlive its client is ended
// by that write, which is its client leaving and not its handler failing.

require_once __DIR__ . '/write_cancel_probe.php';

set_error_handler(null);

// Declared once per worker: a class outlives the request that declared it.
if (!class_exists('OxphpStreamSessionEcho', false)) {
    final class OxphpStreamSessionEcho implements SessionHandlerInterface
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
            OxphpWriteCancelProbe::$stage = 'session-write';
            echo "data: written as the session is\n\n";
            OxphpWriteCancelProbe::$stage = 'past-session-write';

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
}

// The header alone makes the request a stream; nothing is sent before the park,
// so the client leaving is noticed while it is parked (see
// fixture_stream_cancel_ends_it.php).
header('Content-Type: text/event-stream');

session_set_save_handler(new OxphpStreamSessionEcho(), false);
session_id('oxphpstreamsessionecho');
session_start();
$_SESSION['n'] = 1;

OxphpWriteCancelProbe::$stage = 'parked';
sleep(2);

OxphpWriteCancelProbe::$stage = 'returned';
