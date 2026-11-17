<?php

declare(strict_types=1);

// Inner request for fibers/test_serialize_state_after_a_late_fatal and
// fibers/test_serialize_state_on_the_worker_after_a_fatal.
//
// ?role=shutdown stores one object under two session keys, then fails inside
// serialize() in a shutdown function. The session is written after the shutdown
// functions, and its second key is written as a back-reference to the first.
//
// ?role=read opens the same session and answers whether the two keys are still
// one object.
//
// ?role=session_close fails inside serialize() while the session is being
// written; the save handler's close(), which the worker still calls after that,
// records what serialize() and unserialize() do there.
//
// ?role=drain dispatches a task it does not wait for, closing over an object
// that is released on the worker, outside any request, once the task has ended.
// With ?what=fatal the object fails inside serialize() as it is released;
// otherwise it records what serialize() and unserialize() do there.

require_once __DIR__ . '/late_fatal_probe.php';

ini_set('display_errors', '0');

$role = (string) ($_GET['role'] ?? '');
$sid = preg_replace('/[^A-Za-z0-9]/', '', (string) ($_GET['sid'] ?? ''));

if ($role === 'drain') {
    $file = (string) ($_GET['file'] ?? '');
    $fails = ($_GET['what'] ?? '') === 'fatal';
    // The task outlives this request, so what it closed over is released by the
    // worker after the request has ended, not by the request's own end —
    // provided it has started by then: one the request's end finds unstarted is
    // dropped on the spot, its closure with it. It catches the cancellation the
    // request's end delivers, so it ends when its time is up.
    $box = null;
    oxphp_async(static function () use (&$box, $file): int {
        file_put_contents("$file.started", '1');
        $until = microtime(true) + 0.3;
        while (microtime(true) < $until) {
            try {
                usleep(10_000);
            } catch (\Throwable) {
            }
        }

        return 1;
    });
    for ($i = 0; $i < 100 && !is_file("$file.started"); $i++) {
        usleep(20_000);
    }
    // Assigned after the dispatch, which checks only what the closure captured
    // then; the closure is now the only holder.
    $box = $fails ? new OxphpLateFatalOnRelease("$file.fatal") : new OxphpLateProbeOnRelease($file);
    unset($box);
    echo 'dispatched';

    return;
}

ini_set('session.use_cookies', '0');

if ($role === 'session_close') {
    session_set_save_handler(new class implements SessionHandlerInterface {
        public function open(string $path, string $name): bool
        {
            return true;
        }

        public function close(): bool
        {
            OxphpLateFatalProbe::$close = oxphp_late_probe();

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

        public function gc(int $max_lifetime): int
        {
            return 0;
        }

        public function create_sid(): string
        {
            return bin2hex(random_bytes(16));
        }

        public function validateId(string $id): bool
        {
            return true;
        }
    }, false);
    session_id($sid);
    session_start();
    $_SESSION['bad'] = new OxphpLateFatalBadSleep();
    // Taken off by the handler itself before it fails, as in
    // oxphp_late_fatal_inside_serialize().
    set_error_handler(static function (): bool {
        restore_error_handler();
        engine_fatal('OxphpLateFatalSessionWriteBroken');
    });
    echo 'answered';

    return;
}

if ($role === 'read') {
    // The raw data is read before session_start(), which destroys a session it
    // cannot decode. Its warning about that is recorded rather than handed to the
    // handler installed on the worker, which may be a test's that throws.
    $raw = (string) @file_get_contents(session_save_path() . "/sess_$sid");
    $warnings = [];
    set_error_handler(static function (int $type, string $message) use (&$warnings): bool {
        $warnings[] = $message;

        return true;
    });
    session_id($sid);
    session_start();
    restore_error_handler();
    $a = $_SESSION['a'] ?? null;
    $b = $_SESSION['b'] ?? null;
    echo json_encode([
        'a' => get_debug_type($a),
        'b' => get_debug_type($b),
        'same' => is_object($a) && $a === $b,
        'raw' => $raw,
        'warnings' => $warnings,
    ]);

    return;
}

session_id($sid);
session_start();

$o = new \stdClass();
$o->v = 'x';
$_SESSION['a'] = $o;
$_SESSION['b'] = $o;
register_shutdown_function(static function (): void {
    oxphp_late_fatal_inside_serialize('OxphpLateFatalShutdownBroken');
});
echo 'answered';
