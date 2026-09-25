<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/session_inner_request.php';

// The other side of fibers/session_does_not_outlive_its_request.
//
// Session state is given back when the worker takes a request whose predecessor
// has ended — and only then. A request that is still alive, parked on a read
// with its session open, is not a predecessor, and the request admitted beside
// it must not be served by tearing that session down: the parked request would
// resume with no id, no $_SESSION and a store entry it never finished writing.
//
// So this test parks with a session open and checks it is all still there on the
// other side of the suspension. The second half records what the request
// admitted in that window saw, which is the boundary the documentation states:
// one session per worker thread means the two genuinely share it while they
// overlap, and nothing here separates them. If that ever stops being true, this
// is the test that says the documentation needs rewriting.

$mine = str_repeat('e5', 16);

// PHP 8.6 turns session.use_strict_mode on by default, and strict mode swaps an
// id the store has never seen for a fresh one. The id here is named by the test
// itself, which reads it back, so it has to be kept.
ini_set('session.use_strict_mode', '0');

session_id($mine);
session_start();
$_SESSION['who'] = 'OUTER';

// Parks this request on the read. The fixture starts no session of its own, so
// everything it reports about one is this request's.
$peek = json_decode(
    session_inner_request('/tests/fibers/fixture_session_peek.php', str_repeat('f6', 16)),
    true
);

$idAfter = session_id();
$whoAfter = $_SESSION['who'] ?? 'gone';
$statusAfter = session_status();

$t = new TestCase('session_survives_its_own_suspension', 'fibers');

$t->meta('peek', $peek);

$t->assertTrue('the request admitted during the suspension answered with JSON', is_array($peek));

$t->assertSame('this request resumes still holding its own session id', $idAfter, $mine);
$t->assertSame('and its own data behind it', $whoAfter, 'OUTER');
$t->assertSame('with the session still active', $statusAfter, PHP_SESSION_ACTIVE);

if (is_array($peek)) {
    $t->assertSame(
        'the request admitted beside it shares that session, which is the '
            . 'documented boundary of one session per worker thread',
        $peek['session_id'] ?? null,
        $mine
    );
    $t->assertTrue('and reads $_SESSION through it', $peek['session_isset'] ?? false);
}

session_write_close();

$t->done();
