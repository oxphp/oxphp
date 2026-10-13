<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/session_inner_request.php';

// A request that parked before another client's session was opened on the
// worker does not wake into that session, and does not open its own until that
// one has closed.
//
// Keeping new requests off the worker while a session is open is not enough on
// its own: a request already parked when the session appeared would wake in the
// middle of it, read the other client's $_SESSION, and have its own
// session_start() answered with that session. So the open session is taken off
// the thread while its request is parked, and a request that wakes in that
// window runs — it is not held up behind the session — on a thread with no
// session on it. Only its session_start() waits, because one session at a time
// is open on a worker, and it is handed the slot as soon as the holder closes
// rather than at its next poll.
//
// Three requests park first, one in each kind of wait the worker resumes from —
// a timer, a descriptor and a promise — each due 0.4s later. Then a holder
// opens a session and keeps it open for a second, across all three due times.
// Each parked request must have woken inside the hold, found nothing of the
// holder's session, and had its own session_start() return only once the holder
// had closed its session, and promptly after.
//
// The holder also reports the CPU time its worker thread spent while it held
// the session. The three wait in session_start() through most of it, and a wait
// that kept the worker busy instead of parking would burn the rest of the
// hold.

$holderId = str_repeat('3c', 16);
$ids = [
    'sleep' => str_repeat('4d', 16),
    'io' => str_repeat('5e', 16),
    'await' => str_repeat('6f', 16),
];

unset($sharedState['session_parked_before_it']);
$socks = [];
foreach ($ids as $kind => $id) {
    $socks[$kind] = session_inner_send(
        '/tests/fibers/fixture_session_parked_before_it.php?kind=' . $kind,
        $id,
        8.0
    );
}

// Waiting parks this request, which is what lets the worker take the three. The
// holder is sent only once all of them are parked; the ceiling bounds a run in
// which they never get there, and the premise assertions below then say so.
$deadline = microtime(true) + 3.0;
while (count($sharedState['session_parked_before_it'] ?? []) < count($ids) && microtime(true) < $deadline) {
    oxphp_usleep(10_000);
}

$holder = json_decode(
    session_inner_request('/tests/fibers/fixture_session_held_open.php', $holderId, 8.0),
    true
);
$parked = [];
foreach ($socks as $kind => $sock) {
    $parked[$kind] = json_decode(session_inner_read($sock, 8.0), true);
}

$t = new TestCase('session_does_not_reach_a_request_parked_before_it', 'fibers');

$t->meta('holder', $holder);
$t->meta('parked', $parked);

$t->assertTrue('the holder answered with JSON', is_array($holder));
if (!is_array($holder)) {
    $t->done();
}
$t->assertSame('the holder ran under its own client\'s session', $holder['id'] ?? null, $holderId);

foreach ($ids as $kind => $id) {
    $p = $parked[$kind];
    $t->assertTrue("[$kind] answered with JSON", is_array($p));
    if (!is_array($p)) {
        continue;
    }

    // The premise: it parked before the session was opened, and its wait came
    // due while the session was open.
    $t->assertLessThan(
        "[$kind] parked before the holder opened its session",
        (float) ($p['parked_at'] ?? INF),
        (float) ($holder['opened_at'] ?? 0)
    );
    $t->assertGreaterThan(
        "[$kind] its wait came due after the session was opened",
        (float) ($p['due_at'] ?? 0),
        (float) ($holder['opened_at'] ?? INF)
    );
    $t->assertLessThan(
        "[$kind] and before the holder was done with it",
        (float) ($p['due_at'] ?? INF),
        (float) ($holder['done_at'] ?? 0)
    );

    // The mechanism: it ran inside the hold, on a thread with no session on it.
    $t->assertLessThan(
        "[$kind] it woke while the holder still had its session open",
        (float) ($p['resumed_at'] ?? INF),
        (float) ($holder['done_at'] ?? 0)
    );
    $t->assertSame("[$kind] it woke to no session id", $p['pre']['session_id'] ?? null, '');
    $t->assertFalse("[$kind] and no \$_SESSION", $p['pre']['session_isset'] ?? true);
    $t->assertSame("[$kind] and nothing of the holder's", $p['pre']['who'] ?? null, 'none');

    // And its own session waited for the holder's to close, then followed it at
    // once.
    $t->assertGreaterThan(
        "[$kind] its session_start() returned only once the holder had closed its session",
        (float) ($p['started_at'] ?? 0),
        (float) ($holder['closed_at'] ?? INF)
    );
    $t->assertLessThan(
        "[$kind] and within 0.1s of it",
        (float) ($p['started_at'] ?? INF) - (float) ($holder['closed_at'] ?? 0),
        0.1
    );
    $t->assertNull(
        "[$kind] its own session_start() was not refused as already active",
        array_key_exists('notice', $p) ? $p['notice'] : 'the fixture reported no notice field'
    );
    $t->assertSame("[$kind] and opened its own client's session", $p['id'] ?? null, $id);
    $t->assertSame("[$kind] with nothing of the holder's in it", $p['who'] ?? null, 'none');
}

// Not a spin while the session was open. A wait for the session that kept the
// worker busy would burn the rest of the hold — over half a second of the one
// second — while one that parks spends a small fraction of it.
$t->assertLessThan(
    'the worker did not spin while the session was open',
    (float) ($holder['cpu_during_hold'] ?? INF),
    0.3
);

$t->done();
