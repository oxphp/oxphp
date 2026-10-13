<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/session_inner_request.php';

// A request waiting in session_start() for another request's session to close
// is ended by its own time limit there, without getting a session, rather than
// when the other session closes.
//
// The wait parks the request a quarter of a second at a time. A request is
// handed its spent limit when it is resumed, but only PHP code can act on it,
// and a wait that just went back to sleep would carry it until the slot came
// free — then open its session, and only then be ended. So a wait that resumes
// out of time goes back to PHP at once, where the engine ends the request at
// the call.
//
// The waiter parks first, so the worker has taken it before the holder opens a
// session; it wakes inside a hold of two and a half seconds, sets a one-second
// limit and calls session_start().

$holderId = str_repeat('7a', 16);
$waiterId = str_repeat('8b', 16);

unset($sharedState['session_wait_time_limit']);
$waiter = session_inner_send('/tests/fibers/fixture_session_wait_time_limit.php', $waiterId, 8.0);

// Waiting parks this request, which is what lets the worker take the waiter.
$deadline = microtime(true) + 3.0;
while (!isset($sharedState['session_wait_time_limit']['parked_at']) && microtime(true) < $deadline) {
    oxphp_usleep(10_000);
}

$holder = json_decode(
    session_inner_request('/tests/fibers/fixture_session_held_open.php?hold=2.5', $holderId, 8.0),
    true
);
stream_set_timeout($waiter, 8);
$waiterRaw = (string) stream_get_contents($waiter);
fclose($waiter);
$w = $sharedState['session_wait_time_limit'] ?? [];

$t = new TestCase('session_wait_is_ended_by_the_time_limit', 'fibers');

$t->meta('holder', $holder);
$t->meta('waiter', $w);
$t->meta('waiter_status_line', strtok($waiterRaw, "\r\n"));

$t->assertTrue('the holder answered with JSON', is_array($holder));
if (!is_array($holder)) {
    $t->done();
}
$t->assertSame('the holder ran under its own client\'s session', $holder['id'] ?? null, $holderId);

// The premise: the waiter was on the worker before the session was opened, and
// called session_start() while it was held.
$t->assertLessThan(
    'the waiter parked before the holder opened its session',
    (float) ($w['parked_at'] ?? INF),
    (float) ($holder['opened_at'] ?? 0)
);
$t->assertGreaterThan(
    'it set its limit and called session_start() after the session was opened',
    (float) ($w['limit_set_at'] ?? 0),
    (float) ($holder['opened_at'] ?? INF)
);
$t->assertLessThan(
    'and while the holder still had it open',
    (float) ($w['limit_set_at'] ?? INF),
    (float) ($holder['done_at'] ?? 0)
);

// The mechanism: its limit ended it in the wait, before it had a session.
$t->assertTrue('its limit is what ended it: its shutdown functions saw the timeout bit', $w['timed_out'] ?? false);
$t->assertFalse('session_start() never returned to it', isset($w['returned_at']));
$t->assertSame(
    'it never had a session open',
    $w['session_status'] ?? null,
    PHP_SESSION_NONE
);
$t->assertLessThan(
    'it was ended while the holder still had its session open',
    (float) ($w['ended_at'] ?? INF),
    (float) ($holder['done_at'] ?? 0)
);
$t->assertLessThan(
    'and within half a second of its limit',
    (float) ($w['ended_at'] ?? INF) - (float) ($w['limit_set_at'] ?? 0),
    1.5
);
$t->assertContains('it was answered as a request out of time', (string) strtok($waiterRaw, "\r\n"), ' 504');

$t->done();
