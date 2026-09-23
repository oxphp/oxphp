<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/session_inner_request.php';
require_once __DIR__ . '/session_write_probe.php';

// A save handler may use a Fiber of its own while the session is written as its
// request ends.
//
// That write must not park the request — see
// test_session_write_keeps_the_worker.php — but keeping it from parking is the
// server's business, not the engine's: blocking fiber switching outright would
// make Fiber::start() throw in a handler whose store client runs on fibers, and
// the session would not be written.

OxphpSessionWriteProbe::reset();

$sock = session_inner_send('/tests/fibers/fixture_session_write_runs_a_fiber.php', str_repeat('7c', 16));
$body = session_inner_read($sock);

$t = new TestCase('session_write_may_run_a_fiber', 'fibers');

$t->meta('body', $body);
$t->assertSame('the handler ran its fiber and wrote the session',
    OxphpSessionWriteProbe::$written, 'who|s:5:"fiber";|resumed');

$t->done();
