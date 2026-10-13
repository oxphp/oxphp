<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/session_inner_request.php';

// A save handler that fails to close does not leave the worker treating every
// request after it as one with its session open.
//
// A SessionHandler subclass whose close() throws before the files handler
// underneath it has closed leaves that handler's state on the thread, and
// PHP's own close at the end of the request does not reach it: the userland
// handler records itself closed before the exception leaves it. Upstream that
// state is dropped as the next request starts, which a worker does not run, so
// the end of the request drops it. Left standing it reads as an open save
// handler, and a request that parks — this one, waiting for the next inner
// request below — would be taken for one holding its session open: the worker
// would take no new request until it resumed, and with PHP_WORKERS=1 the inner
// request it waits for would never be served.

$t = new TestCase('session_close_that_throws_is_not_left_open', 'fibers');

$closeThrows = json_decode(session_inner_request(
    '/tests/fibers/fixture_session_close_throws.php?role=close-throws',
    str_repeat('d0', 16),
    5.0
), true);
$t->meta('close_throws', $closeThrows);

// The premise: the close threw, with the session already marked closed.
$t->assertTrue('the request whose close throws answered with JSON', is_array($closeThrows));
$t->assertTrue('its session_write_close() threw', $closeThrows['threw'] ?? false);
$t->assertSame('with the session marked closed after it', $closeThrows['status_after'] ?? null, PHP_SESSION_NONE);

$sentAt = microtime(true);
$plain = json_decode(session_inner_request(
    '/tests/fibers/fixture_session_close_throws.php?role=plain',
    str_repeat('e1', 16),
    3.0
), true);
$t->meta('plain', $plain);
$t->meta('sent_at', $sentAt);

$t->assertTrue('the next request was served while this one waited for it', is_array($plain));
$t->assertLessThan(
    'promptly',
    (float) ($plain['served_at'] ?? INF) - $sentAt,
    1.0
);

$t->done();
