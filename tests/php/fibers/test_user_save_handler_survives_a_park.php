<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// A save handler a request registers is still the one its session uses after
// the request has parked.
//
// session_set_save_handler() records itself as session.save_handler = "user",
// and the session module accepts that value from session_set_save_handler()
// alone. A worker that took the directive off the request at a park and applied
// it again on resume would be refused, and the session started afterwards would
// quietly go to the default files handler — which to an application storing
// sessions elsewhere looks like every user being logged out.

$t = new TestCase('user_save_handler_survives_a_park', 'fibers');

$handler = new class implements SessionHandlerInterface {
    public bool $opened = false;

    public function open(string $path, string $name): bool
    {
        $this->opened = true;
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
};

session_set_save_handler($handler, false);
$t->assertSame('the handler is registered before the park', ini_get('session.save_handler'), 'user');

// Hooked in this profile: parks this request and hands the worker back.
usleep(1000);

$t->assertSame('and is still the save handler after it', ini_get('session.save_handler'), 'user');

// No cookie: nothing in this test sends headers, and none are needed.
ini_set('session.use_cookies', '0');
session_id('oxphpsavehandlerpark');
session_start();
session_write_close();

$t->assertTrue('and the session started after the park went to it', $handler->opened);

$t->done();
