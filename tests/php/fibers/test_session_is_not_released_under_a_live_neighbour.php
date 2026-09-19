<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/session_inner_request.php';

// Session state is one set per worker thread, so the worker can only give it
// back when no request it is carrying is still working in it. Asking who OPENED
// the session is not enough to know that, and this is the case that shows why:
// the opener can be the first to leave.
//
// Three inner requests on one worker, which this outer request drives by parking
// on each read:
//
//   1. the OPENER starts a session and parks for a short while;
//   2. the NEIGHBOUR is admitted in that window and is handed the opener's
//      session — the documented overlap — and parks for much longer;
//   3. the opener finishes and leaves, with the neighbour still parked in that
//      session;
//   4. a THIRD request is admitted into that window, which is where the worker
//      decides whether the session can be given back.
//
// A worker that answers step 4 by asking whether the opener is still around says
// yes it can, and takes the session away from a request that is still in it: the
// neighbour comes back to no session id, no $_SESSION and its writes gone to the
// store under the opener's client's id. So the neighbour reports what it was
// working in before it parked and what it found when it woke, and those two have
// to be the same thing.
//
// The third request reports from the other side: it is the one whose arrival
// could trigger the release, and on a worker that holds the session correctly it
// sees that session standing.

$openerId = str_repeat('7f', 16);

// Sent before either is read: reading is what parks this request, and the worker
// only takes an inner request once this one is parked. Both are queued first so
// the neighbour is admitted while the opener holds the session rather than after
// it has gone.
$openerSock = session_inner_send('/tests/fibers/fixture_session_opens_then_parks_briefly.php', $openerId);
$neighbourSock = session_inner_send('/tests/fibers/fixture_session_neighbour_holds_it.php', $openerId);

// Parks here; comes back when the opener has finished, with the neighbour still
// parked in the session the opener left open.
$opener = session_inner_read($openerSock);

// Into exactly that window.
$third = json_decode(
    session_inner_request('/tests/fibers/fixture_session_peek.php', str_repeat('8e', 16)),
    true
);

$neighbour = json_decode(session_inner_read($neighbourSock, 8.0), true);

$t = new TestCase('session_is_not_released_under_a_live_neighbour', 'fibers');

$t->meta('opener_body', $opener);
$t->meta('third', $third);
$t->meta('neighbour', $neighbour);

$t->assertContains('the opener ran and finished first', $opener, 'OPENER-DONE:' . $openerId);
$t->assertTrue('the neighbour answered with JSON', is_array($neighbour));
$t->assertTrue('the third request answered with JSON', is_array($third));

if (!is_array($neighbour) || !is_array($third)) {
    $t->done();
}

// The premise: the neighbour really was working in the opener's session before
// it parked. Without this the assertions below are satisfied by a neighbour that
// never had a session to lose.
$t->assertSame(
    'the neighbour was admitted into the session the opener had open',
    $neighbour['before']['session_id'] ?? null,
    $openerId
);
$t->assertSame(
    'and was reading what the opener had put in it',
    $neighbour['before']['who'] ?? null,
    'the request that opened it'
);

// The finding.
$t->assertSame(
    'the session is still the neighbour\'s when it wakes, although the request '
        . 'that opened it has gone and another has been taken since',
    $neighbour['after']['session_id'] ?? null,
    $openerId
);
$t->assertSame(
    'with what was in it still there',
    $neighbour['after']['who'] ?? null,
    'the request that opened it'
);
$t->assertTrue(
    'and $_SESSION still defined',
    $neighbour['after']['session_isset'] ?? false
);

// From the other side: the request whose arrival is the moment the worker
// decides. It sees the session held rather than given back.
$t->assertSame(
    'the request admitted in that window finds the session still standing, '
        . 'which is the worker declining to give it back',
    $third['session_id'] ?? null,
    $openerId
);

$t->done();
