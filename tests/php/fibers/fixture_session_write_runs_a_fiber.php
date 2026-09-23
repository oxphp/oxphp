<?php

declare(strict_types=1);

// Inner request for test_session_write_may_run_a_fiber.php: a save handler whose
// write drives a userland Fiber, as a store client built on one does. What the
// write managed is left in OxphpSessionWriteProbe::$written for the test to read.

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
        try {
            $fiber = new Fiber(static function (string $data): string {
                return $data . '|' . Fiber::suspend('suspended');
            });
            $fiber->start($data);
            $fiber->resume('resumed');
            OxphpSessionWriteProbe::$written = $fiber->getReturn();
        } catch (Throwable $e) {
            OxphpSessionWriteProbe::$written = get_class($e) . ': ' . $e->getMessage();
        }
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
session_id('oxphpsessionwriterunsafiber');
session_start();
$_SESSION['who'] = 'fiber';

echo 'left open';
