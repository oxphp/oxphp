<?php

declare(strict_types=1);

// Inner request for
// fibers/test_session_release_after_a_fiber_fatal_keeps_the_request_fiber.
//
// A save handler whose write() fatals inside a Fiber it starts. The engine
// hands that fatal to the request's own fiber, and ending a fiber that way
// clears the engine's record of the current fiber. The worker goes on
// releasing the session after it: the handler is still marked as running, so
// the close that follows is refused with a warning, and the warning is handed
// to the error handler installed below, which records the fiber it runs in.

require_once __DIR__ . '/recovery_probe.php';
require_once __DIR__ . '/../engine_fatal.php';

// Left installed: the outer request takes it off once it has read the probe.
set_error_handler(static function (int $errno, string $message): bool {
    if (str_contains($message, 'recursive manner')) {
        $current = \Fiber::getCurrent();
        OxphpRecoveryProbe::$warned = true;
        OxphpRecoveryProbe::$fiberAtWarning = $current === null ? null : spl_object_id($current);
    }

    return true;
});

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
        (new \Fiber(static function (): void {
            engine_fatal('OxphpRecoverySessionWriteFiberFatal');
        }))->start();

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
session_id('oxphprecoverysessionwritefatal');
session_start();
$_SESSION['who'] = 'nested-fiber-fatal';

$current = \Fiber::getCurrent();
OxphpRecoveryProbe::$requestFiber = $current === null ? null : spl_object_id($current);

echo 'left open';
