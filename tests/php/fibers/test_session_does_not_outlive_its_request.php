<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/session_inner_request.php';

// Session state — the active flag, the id, the array and the _SESSION entry in
// the symbol table — is one set per worker thread, and only the reset on the
// serve loop's blocking path gives it back. A request admitted by the event loop
// goes through a different preparation, which never touched it: it started with
// whatever the request before it left standing.
//
// This test drives both shapes that reaches a request through, from one outer
// request that parks on the read of each inner one. The park is what makes the
// inner requests event-loop admissions rather than blocking ones, and with one
// worker in this profile a body coming back at all proves they were served by
// the thread this test is running on.
//
//   1. The predecessor left its session OPEN. Upstream session_start() answers
//      TRUE on an already-active session, raising a notice and reading neither
//      the cookie nor the store, so the probe would be handed the predecessor's
//      array under its own name.
//   2. The predecessor CLOSED its session with session_write_close(). That marks
//      the session none but leaves its id installed, and upstream consults the
//      cookie only when no id is installed — so the probe would adopt the
//      predecessor's session id, read its data out of the store, and be sent that
//      id back in a Set-Cookie of its own.
//
// Both are asserted on the mechanism rather than on the data alone: the notice
// for the first, the session id the probe ends up with for the second.

// Session ids rather than arbitrary strings: 32 hex characters is what this
// build's session.sid_length and session.sid_bits_per_character produce, so the
// store accepts and creates files for them exactly as it would for a real one.
$seedOpenId = str_repeat('a1', 16);
$probeAfterOpenId = str_repeat('b2', 16);
$seedClosedId = str_repeat('c3', 16);
$probeAfterClosedId = str_repeat('d4', 16);

$seedOpen = session_inner_request('/tests/fibers/fixture_session_leave_open.php', $seedOpenId);
$afterOpen = json_decode(
    session_inner_request('/tests/fibers/fixture_session_probe.php', $probeAfterOpenId),
    true
);

$seedClosed = session_inner_request('/tests/fibers/fixture_session_close_and_leave_id.php', $seedClosedId);
$afterClosed = json_decode(
    session_inner_request('/tests/fibers/fixture_session_probe.php', $probeAfterClosedId),
    true
);

$t = new TestCase('session_does_not_outlive_its_request', 'fibers');

$t->meta('seed_open_body', $seedOpen);
$t->meta('seed_closed_body', $seedClosed);

$t->assertContains('the predecessor that leaves its session open ran', $seedOpen, 'SEEDED-ACTIVE:');
$t->assertContains('the predecessor that closes its session ran', $seedClosed, 'SEEDED-CLOSED:');
$t->assertTrue('the probe after the open session answered with JSON', is_array($afterOpen));
$t->assertTrue('the probe after the closed session answered with JSON', is_array($afterClosed));

if (!is_array($afterOpen) || !is_array($afterClosed)) {
    $t->done();
}

// ── The predecessor left its session open ────────────────────

// array_key_exists rather than ??, because null is the answer this asserts: a
// probe that reported no notice and a probe that reported no notice field would
// otherwise be the same value here, and only one of them is the pass.
$t->assertNull(
    'session_start() is not refused as already active, which is what a request '
        . 'handed the previous one is told',
    array_key_exists('notice', $afterOpen) ? $afterOpen['notice'] : 'the probe reported no notice field'
);
$t->assertTrue(
    'and it does start a session, so the id and the data below are a session\'s',
    $afterOpen['started'] ?? false
);
$t->assertSame(
    'the request gets the session its own cookie names',
    $afterOpen['id'] ?? null,
    $probeAfterOpenId
);
$t->assertSame(
    'and not the data the request before it put there',
    $afterOpen['who'] ?? null,
    'none'
);
$t->assertFalse(
    '$_SESSION is undefined until this request starts a session of its own',
    $afterOpen['pre_session_isset'] ?? true
);
$t->assertSame(
    'and no session id is installed before it either',
    $afterOpen['pre_session_id'] ?? null,
    ''
);

// ── The predecessor closed its session ───────────────────────

$t->assertNull(
    'session_start() is not refused after a predecessor that closed its session',
    array_key_exists('notice', $afterClosed) ? $afterClosed['notice'] : 'the probe reported no notice field'
);
$t->assertTrue(
    'and it does start a session here too',
    $afterClosed['started'] ?? false
);
$t->assertSame(
    'the request is not adopted into the id its predecessor left installed',
    $afterClosed['id'] ?? null,
    $probeAfterClosedId
);
$t->assertSame(
    'so it does not read that session out of the store',
    $afterClosed['who'] ?? null,
    'none'
);
$t->assertFalse(
    '$_SESSION does not survive a predecessor that closed its session either',
    $afterClosed['pre_session_isset'] ?? true
);
$t->assertSame(
    'nor does its id',
    $afterClosed['pre_session_id'] ?? null,
    ''
);

$t->meta('after_open', $afterOpen);
$t->meta('after_closed', $afterClosed);

$t->done();
