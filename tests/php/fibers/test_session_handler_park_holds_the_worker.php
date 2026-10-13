<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/session_inner_request.php';

// A request parked inside its own session_start() — in the read of a save
// handler that talks to its store over a connection that suspends — already
// has its session open: the session module marks it active before it opens the
// handler and reads. So the worker takes no new request until it is closed, as
// for any other park with a session open, and the window counts from the moment
// the session is started rather than from when it is filled. That is the
// ordinary shape of a database or cache session handler under RUNTIME_HOOKS.

$holderId = str_repeat('a7', 16);

// The holder's read parks for 0.6s straight away; this is sent into it.
$arrival = session_arrival_send('/tests/fibers/fixture_session_probe.php', 0.2);

$holder = json_decode(
    session_inner_request('/tests/fibers/fixture_session_handler_read_parks.php', $holderId, 8.0),
    true
);
$sent = oxphp_async_await($arrival, 10.0);
$probe = json_decode($sent['body'] ?? '', true);

$t = new TestCase('session_handler_park_holds_the_worker', 'fibers');

$t->meta('holder', $holder);
$t->meta('arrival', $sent);

$t->assertTrue('the holder answered with JSON', is_array($holder));
$t->assertTrue('the arrival answered with JSON', is_array($probe));

if (!is_array($holder) || !is_array($probe)) {
    $t->done();
}

$t->assertSame('the holder ran under its own client\'s session', $holder['id'] ?? null, $holderId);

// The premise: the arrival was sent while the holder was parked in its read.
$t->assertGreaterThan(
    'the arrival was sent after the holder\'s read had parked',
    (float) ($sent['sent_at'] ?? 0),
    (float) ($holder['read_entered_at'] ?: INF)
);
$t->assertLessThan(
    'and before the read returned',
    (float) ($sent['sent_at'] ?? INF),
    (float) ($holder['read_left_at'] ?? 0)
);

// The mechanism.
$t->assertGreaterThan(
    'the arrival began to run only once the holder was done with its session',
    (float) ($probe['started_at'] ?? 0),
    (float) ($holder['done_at'] ?? INF)
);

$t->assertSame('it found no session id standing as it began', $probe['pre_session_id'] ?? null, '');
$t->assertNull(
    'its session_start() was not refused as already active',
    array_key_exists('notice', $probe) ? $probe['notice'] : 'the probe reported no notice field'
);
$t->assertNotEmpty('it was given a session id', $probe['id'] ?? '');
$t->assertNotEqual('of its own, not the holder\'s', $probe['id'] ?? $holderId, $holderId);
$t->assertNotContains('and the cookie it was sent does not carry the holder\'s', (string) ($sent['headers'] ?? ''), $holderId);

$t->done();
