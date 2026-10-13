<?php

declare(strict_types=1);

// Two roles around a session the end of a request opens again after that
// request's own session has been given back, for the test that it is still
// written and closed through the handler it was opened with.
//
// ?role=holder opens its session under the files handler and parks for a
// second with it open, so that the session leaves the thread with that handler.
// On resume it closes the session and raises an E_USER_ERROR its own error
// handler takes over, from a call given an object nothing else holds. From PHP
// 8.5 the fatal's backtrace keeps that call's arguments, and the end of the
// request lets go of it only after it has written the session — so the object's
// destructor, which opens the session again under the same id and leaves it
// open, runs there. Before 8.5 it runs as the call returns.
//
// By then the response, headers included, has gone out, and the session module
// refuses a session that would need one: a cookie to carry the id, or the cache
// headers session.cache_limiter sends. Nor will it change its settings any more.
// So the holder runs its session without either from the start, with the id
// given rather than read from the cookie.
//
// ?role=neighbour parks first, wakes inside that second, registers a userland
// save handler of its own and ends, so that the thread carries a handler other
// than the one the holder's session was opened with.
//
// Every call into the neighbour's handler is appended to ?log= as one JSON
// line, and so is what the destructor saw.

set_error_handler(null);
set_exception_handler(null);

$role = (string) ($_GET['role'] ?? '');
$log = (string) ($_GET['log'] ?? '');

if (!class_exists('OxphpSessionReopenNeighbour', false)) {
    final class OxphpSessionReopenNeighbour implements SessionHandlerInterface
    {
        public function __construct(private string $log)
        {
        }

        private function note(string $call, string $id = ''): void
        {
            file_put_contents($this->log, json_encode(['tag' => 'neighbour', 'call' => $call, 'id' => $id]) . "\n", FILE_APPEND);
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

    final class OxphpSessionReopener
    {
        public function __construct(private string $id, private string $log)
        {
        }

        public function __destruct()
        {
            $before = ['id' => session_id(), 'status' => session_status()];
            session_id($this->id);
            $started = session_start();
            $_SESSION['reopened'] = 'by a destructor';
            file_put_contents($this->log, json_encode([
                'tag' => 'destructor',
                'call' => 'reopened',
                'id' => session_id(),
                'before' => $before,
                'started' => $started,
                'module' => session_module_name(),
            ]) . "\n", FILE_APPEND);
        }
    }
}

// PHP 8.6 turns session.use_strict_mode on by default; the ids here are the
// clients' cookies and the test reads them back.
ini_set('session.use_strict_mode', '0');

header('Content-Type: application/json');

if ($role === 'holder') {
    ini_set('session.save_handler', 'files');
    // The backtrace keeps the arguments only where they are not ignored.
    ini_set('zend.exception_ignore_args', '0');
    ini_set('session.use_cookies', '0');
    ini_set('session.cache_limiter', '');
    session_id((string) ($_COOKIE[session_name()] ?? ''));
    session_start();
    $_SESSION['who'] = 'holder';
    $id = session_id();
    $openedAt = microtime(true);

    oxphp_sleep(1.0);

    $doneAt = microtime(true);
    session_write_close();

    // Nothing may raise an error after this one: any error lets go of the
    // backtrace the fatal left.
    set_error_handler(static fn (): bool => true);
    (static function (OxphpSessionReopener $reopener): void {
        trigger_error('oxphp-session-reopen: taken over by the error handler', E_USER_ERROR);
    })(new OxphpSessionReopener($id, $log));
    restore_error_handler();

    echo json_encode([
        'id' => $id,
        'opened_at' => $openedAt,
        'done_at' => $doneAt,
        'save_path' => session_save_path(),
    ]);
    return;
}

if ($role === 'neighbour') {
    $sharedState['session_reopen_parked'] = microtime(true);
    oxphp_sleep(0.4);

    $configured = session_set_save_handler(new OxphpSessionReopenNeighbour($log), false);
    echo json_encode(['configured' => $configured, 'configured_at' => microtime(true)]);
    return;
}

http_response_code(400);
echo "unknown role: $role";
