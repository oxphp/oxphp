<?php

declare(strict_types=1);

// A save handler left open by a failed session_regenerate_id(), for the test
// that it stays with its request like an open session does.
//
// ?role=holder registers a userland save handler whose create_sid() returns no
// id, starts its client's session and regenerates the id. The regeneration
// writes and closes the session, opens the handler again and asks it for an id;
// given none, it marks the session none and throws, with the handler still
// open. The holder then parks for a second.
//
// ?role=neighbour registers a userland save handler of its own and opens its
// client's session.
//
// Every call into either handler, and each one's destruction, is appended to
// ?log= as one JSON line, in the order they happened.

set_error_handler(null);
set_exception_handler(null);

$role = (string) ($_GET['role'] ?? '');
$log = (string) ($_GET['log'] ?? '');
$arrivedAt = microtime(true);

if (!class_exists('OxphpSessionRegenHandler', false)) {
    final class OxphpSessionRegenHandler implements SessionHandlerInterface
    {
        public function __construct(private string $tag, private string $log, private bool $noId)
        {
        }

        private function note(string $call, string $id = ''): void
        {
            file_put_contents($this->log, json_encode([
                'tag' => $this->tag,
                'call' => $call,
                'id' => $id,
            ]) . "\n", FILE_APPEND);
        }

        public function open(string $path, string $name): bool
        {
            $this->note('open');

            return true;
        }

        public function close(): bool
        {
            $this->note('close');

            return true;
        }

        public function read(string $id): string
        {
            $this->note('read', $id);

            return '';
        }

        public function write(string $id, string $data): bool
        {
            $this->note('write', $id);

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

        public function create_sid(): string|false
        {
            $this->note('create_sid');

            return $this->noId ? false : bin2hex(random_bytes(16));
        }

        // PHP 8.6 warns once for each of these a handler class lacks.
        public function validateId(string $id): bool
        {
            return true;
        }

        public function __destruct()
        {
            $this->note('destruct');
        }
    }
}

// PHP 8.6 turns session.use_strict_mode on by default; the ids here are the
// clients' cookies and the test reads them back.
ini_set('session.use_strict_mode', '0');

header('Content-Type: application/json');

if ($role === 'holder') {
    session_set_save_handler(new OxphpSessionRegenHandler('holder', $log, true), false);
    session_start();
    $_SESSION['who'] = 'holder';

    $error = null;
    try {
        session_regenerate_id();
    } catch (\Throwable $e) {
        $error = get_class($e) . ': ' . $e->getMessage();
    }
    $status = session_status();

    $sharedState['session_regen_parked'] = microtime(true);
    oxphp_sleep(1.0);

    echo json_encode([
        'error' => $error,
        'status' => $status,
        'done_at' => microtime(true),
    ]);
    return;
}

if ($role === 'neighbour') {
    $configured = session_set_save_handler(new OxphpSessionRegenHandler('neighbour', $log, false), false);
    session_start();
    $_SESSION['who'] = 'neighbour';
    $id = session_id();
    session_write_close();

    echo json_encode([
        'arrived_at' => $arrivedAt,
        'configured' => $configured,
        'id' => $id,
    ]);
    return;
}

http_response_code(400);
echo "unknown role: $role";
