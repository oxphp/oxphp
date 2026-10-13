<?php

declare(strict_types=1);

// Two roles around one session held open across a park, for the test that the
// session is written and closed through the handler it was opened with.
//
// ?role=holder registers a userland save handler of its own, writes its session
// with the php_serialize serializer, and keeps it open across a park of a
// second. The handler object is held by the session module and nothing else.
//
// With ?variant=again the holder then registers a second handler of its own
// once its session has closed, parks briefly, and opens its session again.
// With ?variant=relift it registers the second handler, opens its session
// through it straight away and parks with it open, then closes it, parks with
// it closed, and opens it a third time.
//
// ?role=neighbour parks first, wakes inside that second, configures the session
// module for itself — ?variant=user registers a userland handler of its own and
// switches the serializer to php_binary; ?variant=files switches the save
// handler to the files one — and then opens its own session. With
// ?variant=again or ?variant=relift it registers a userland handler of its own
// and opens nothing.
//
// Every call into either userland handler, and each one's destruction, is
// appended to ?log= as one JSON line, in the order they happened.

set_error_handler(null);
set_exception_handler(null);

$role = (string) ($_GET['role'] ?? '');
$variant = (string) ($_GET['variant'] ?? '');
$log = (string) ($_GET['log'] ?? '');

if (!class_exists('OxphpSessionPinHandler', false)) {
    final class OxphpSessionPinHandler implements SessionHandlerInterface
    {
        public function __construct(private string $tag, private string $log)
        {
        }

        private function note(string $call, string $id = '', string $data = ''): void
        {
            file_put_contents($this->log, json_encode([
                'tag' => $this->tag,
                'call' => $call,
                'id' => $id,
                'data' => base64_encode($data),
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
            $this->note('write', $id, $data);

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
    ini_set('session.serialize_handler', 'php_serialize');
    session_set_save_handler(new OxphpSessionPinHandler('holder', $log), false);
    session_start();
    $_SESSION['who'] = 'holder';
    $openedAt = microtime(true);

    oxphp_sleep(1.0);

    $doneAt = microtime(true);
    $id = session_id();
    session_write_close();
    $closedAt = microtime(true);

    $again = [];
    if ($variant === 'again') {
        session_set_save_handler(new OxphpSessionPinHandler('holder-again', $log), false);
        $again['set_again_at'] = microtime(true);
        oxphp_sleep(0.05);

        session_start();
        $_SESSION['who'] = 'holder, again';
        $again['id_again'] = session_id();
        session_write_close();
    } elseif ($variant === 'relift') {
        session_set_save_handler(new OxphpSessionPinHandler('holder-again', $log), false);
        $again['set_again_at'] = microtime(true);

        session_start();
        $_SESSION['who'] = 'holder, again';
        $again['id_again'] = session_id();
        oxphp_sleep(0.05);
        session_write_close();
        oxphp_sleep(0.05);

        session_start();
        $_SESSION['who'] = 'holder, a third time';
        $again['id_third'] = session_id();
        session_write_close();
    }

    echo json_encode([
        'id' => $id,
        'opened_at' => $openedAt,
        'done_at' => $doneAt,
        'closed_at' => $closedAt,
    ] + $again);
    return;
}

if ($role === 'neighbour') {
    $sharedState['session_handler_pin_parked'] = microtime(true);
    oxphp_sleep(0.4);

    if ($variant === 'user') {
        $configured = ini_set('session.serialize_handler', 'php_binary') !== false
            && session_set_save_handler(new OxphpSessionPinHandler('neighbour', $log), false);
    } elseif ($variant === 'again' || $variant === 'relift') {
        $configured = session_set_save_handler(new OxphpSessionPinHandler('neighbour', $log), false);
    } else {
        $configured = ini_set('session.save_handler', 'files') !== false;
    }
    $configuredAt = microtime(true);

    if ($variant === 'again' || $variant === 'relift') {
        echo json_encode(['configured' => $configured, 'configured_at' => $configuredAt]);
        return;
    }

    session_start();
    $startedAt = microtime(true);
    $_SESSION['who'] = 'neighbour';
    $id = session_id();
    $module = session_module_name();
    session_write_close();

    echo json_encode([
        'configured' => $configured,
        'configured_at' => $configuredAt,
        'started_at' => $startedAt,
        'id' => $id,
        'module' => $module,
    ]);
    return;
}

http_response_code(400);
echo "unknown role: $role";
