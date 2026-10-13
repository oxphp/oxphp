<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/session_inner_request.php';

// A session closed before its request parks goes with that request, not to the
// request the worker runs in the meantime.
//
// session_write_close() ends the session but leaves its id installed on the
// thread, along with the array and the $_SESSION entry, and session_start()
// takes an installed id before it looks at any cookie. A request admitted while
// the closer is parked would start its session under the closer's id — read
// the closer's data from the store, and send the closer's id back to its own
// client as a cookie. A closed session holds no lock, so the worker does keep
// taking requests while its owner is parked; what it must do is take the
// leftovers off the thread for the park and put them back on resume.
//
// So two things are asserted, and the first is the one that tells this apart
// from simply holding the worker until the closer ends: the arrival runs while
// the closer is still parked. Then neither side sees the other's session.

$closerId = str_repeat('2b', 16);

$arrival = session_arrival_send('/tests/fibers/fixture_session_probe.php', 0.25);

$closer = json_decode(
    session_inner_request('/tests/fibers/fixture_session_closed_then_parks.php', $closerId, 8.0),
    true
);
$sent = oxphp_async_await($arrival, 10.0);
$probe = json_decode($sent['body'] ?? '', true);

$t = new TestCase('session_closed_before_a_park_stays_with_its_request', 'fibers');

$t->meta('closer', $closer);
$t->meta('arrival', $sent);

$t->assertTrue('the closer answered with JSON', is_array($closer));
$t->assertTrue('the arrival answered with JSON', is_array($probe));

if (!is_array($closer) || !is_array($probe)) {
    $t->done();
}

// The premise: the arrival was sent after the closer had closed its session and
// before it woke.
$t->assertGreaterThan(
    'the arrival was sent after the closer had closed its session',
    (float) ($sent['sent_at'] ?? 0),
    (float) ($closer['closed_at'] ?? INF)
);
$t->assertLessThan(
    'and while it was still parked',
    (float) ($sent['sent_at'] ?? INF),
    (float) ($closer['done_at'] ?? 0)
);

// A closed session does not hold the worker.
$t->assertLessThan(
    'the arrival ran while the closer was still parked',
    (float) ($probe['started_at'] ?? INF),
    (float) ($closer['done_at'] ?? 0)
);

$t->assertSame('it found no session id standing as it began', $probe['pre_session_id'] ?? null, '');
$t->assertFalse('and no $_SESSION', $probe['pre_session_isset'] ?? true);
$t->assertNull(
    'its session_start() raised nothing',
    array_key_exists('notice', $probe) ? $probe['notice'] : 'the probe reported no notice field'
);
$t->assertNotEmpty('it was given a session id', $probe['id'] ?? '');
$t->assertNotEqual('of its own, not the closer\'s', $probe['id'] ?? $closerId, $closerId);
$t->assertSame('with nothing of the closer\'s in it', $probe['who'] ?? null, 'none');
$t->assertContains(
    'and the cookie it was sent carries its own id',
    (string) ($sent['headers'] ?? ''),
    'PHPSESSID=' . ($probe['id'] ?? '')
);
$t->assertNotContains('and not the closer\'s', (string) ($sent['headers'] ?? ''), $closerId);

// The other side: the closer wakes to its own closed session, as it left it.
$t->assertSame('the closer woke to its own session id', $closer['id'] ?? null, $closerId);
$t->assertTrue('and its own $_SESSION', $closer['session_isset'] ?? false);
$t->assertSame('with what it wrote still in it', $closer['who'] ?? null, 'the request that closed it');
$t->assertSame('and the session still closed', $closer['status'] ?? null, PHP_SESSION_NONE);

$t->done();
