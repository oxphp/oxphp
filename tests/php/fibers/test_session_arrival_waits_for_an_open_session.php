<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/session_inner_request.php';

// A request that arrives while another client's session is open on the worker
// gets a session of its own.
//
// Session state is one set per worker thread, and session_start() on a thread
// where a session is already active answers TRUE without reading the cookie or
// the store. So a request admitted beside one parked with its session open
// would be handed that session — the other client's id, which then goes back
// out as its own, and everything stored under it. The worker therefore admits
// no new request while a session is parked open on it: the arrival waits in the
// queue until the session is closed, and only then gets a thread with no
// session on it.
//
// The holder opens a session under its client's cookie and parks for a second
// with it open; a request with no cookie at all is sent into that window. What
// it must not see is the holder's id or data — before its own session_start(),
// after it, or in the cookie it is sent — and it must have begun only after the
// holder was done with the session, which is the mechanism rather than an
// outcome a lucky ordering could also produce.

$holderId = str_repeat('1a', 16);

// Sent a quarter of a second from now. The holder is taken as soon as this
// request parks on its read below, so by then its session is open.
$arrival = session_arrival_send('/tests/fibers/fixture_session_probe.php', 0.25);

$holder = json_decode(
    session_inner_request('/tests/fibers/fixture_session_held_open.php', $holderId, 8.0),
    true
);
$sent = oxphp_async_await($arrival, 10.0);
$probe = json_decode($sent['body'] ?? '', true);

$t = new TestCase('session_arrival_waits_for_an_open_session', 'fibers');

$t->meta('holder', $holder);
$t->meta('arrival', $sent);

$t->assertTrue('the holder answered with JSON', is_array($holder));
$t->assertTrue('the arrival answered with JSON', is_array($probe));

if (!is_array($holder) || !is_array($probe)) {
    $t->done();
}

$t->assertSame('the holder ran under its own client\'s session', $holder['id'] ?? null, $holderId);

// The premise: the arrival was sent while the holder had its session open.
// Without it everything below is met by an arrival that simply came later.
$t->assertGreaterThan(
    'the arrival was sent after the holder had opened its session',
    (float) ($sent['sent_at'] ?? 0),
    (float) ($holder['opened_at'] ?? INF)
);
$t->assertLessThan(
    'and before the holder was done with it',
    (float) ($sent['sent_at'] ?? INF),
    (float) ($holder['done_at'] ?? 0)
);

// The mechanism.
$t->assertGreaterThan(
    'the arrival began to run only once the holder was done with its session',
    (float) ($probe['started_at'] ?? 0),
    (float) ($holder['done_at'] ?? INF)
);

$t->assertSame('it found no session id standing as it began', $probe['pre_session_id'] ?? null, '');
$t->assertFalse('and no $_SESSION', $probe['pre_session_isset'] ?? true);
$t->assertNull(
    'its session_start() was not refused as already active',
    array_key_exists('notice', $probe) ? $probe['notice'] : 'the probe reported no notice field'
);
$t->assertNotEmpty('it was given a session id', $probe['id'] ?? '');
$t->assertNotEqual('of its own, not the holder\'s', $probe['id'] ?? $holderId, $holderId);
$t->assertSame('with nothing of the holder\'s in it', $probe['who'] ?? null, 'none');
$t->assertContains(
    'and the cookie it was sent carries its own id',
    (string) ($sent['headers'] ?? ''),
    'PHPSESSID=' . ($probe['id'] ?? '')
);
$t->assertNotContains('and not the holder\'s', (string) ($sent['headers'] ?? ''), $holderId);

$t->done();
