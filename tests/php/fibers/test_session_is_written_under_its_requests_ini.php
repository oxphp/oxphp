<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/fiber_park_registry.php';
require_once __DIR__ . '/session_write_probe.php';

// A session its request leaves open is written under that request's ini
// settings.
//
// PHP writes a session in its request shutdown before it puts the request's ini
// changes back, so the write sees what the request set: the serialize_precision
// its floats are written with, the timeouts a save handler talking to a database
// relies on. A worker that put the settings back first would write the session
// under the baseline instead.
//
// The inner request opens the session and ends while this one is parked, so no
// request but the inner one is in the session: it is the inner request's alone
// to write, at its end. It writes 0.1 + 0.2 at a precision of 5, which serializes
// as 0.3 — and at the default precision as 0.30000000000000004.

$t = new TestCase('session_is_written_under_its_requests_ini', 'fibers');

OxphpSessionWriteProbe::reset();

$body = fiber_inner_request('/tests/fibers/fixture_session_written_under_its_ini.php');

$t->assertSame('the inner request ran', $body, 'left open');
$t->assertSame(
    'and its session was written by the time it had finished, with its own precision',
    OxphpSessionWriteProbe::$written,
    'sum|d:0.3;'
);

$t->done();
