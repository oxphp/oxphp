<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/session_inner_request.php';
require_once __DIR__ . '/session_write_probe.php';

// A session written as its request ends is written without handing the worker
// to anyone else.
//
// Session state is one set per worker thread. A request whose session nobody
// else is in has it written as the request ends, and a save handler that talks
// to a store over the network sleeps or reads inside that write — a point where
// a request can otherwise park. Parking there would let the next request in
// while the session is still on the thread and not yet given back: that request
// would find another client's session id and $_SESSION standing, and the writer
// would then clear them from under it when it woke.
//
// Two inner requests, queued together: the WRITER, whose handler's write sleeps
// on a hooked call, and a PEEK behind it. The peek reports the session it finds
// and whether the writer's write had finished by the time it ran.

OxphpSessionWriteProbe::reset();

// The writer goes first, and the peek only once the writer has reached its
// write. Two requests sent back to back are not taken in the order they were
// sent: each connection is read by a task of its own, and a request joins the
// worker's queue once its headers have been read. A peek taken first runs before
// the writer has a session to write.
//
// Waiting parks this request, which is what lets the worker take the writer, and
// this loop only runs again once the writer gives the worker back: in the middle
// of its write on a build that lets it park there, after the write on one that
// does not. The peek is sent at that point either way. The ceiling only bounds a
// run where the writer never gets that far; the assertions below then say so.
$writerSock = session_inner_send('/tests/fibers/fixture_session_write_parks.php', str_repeat('5a', 16));
$deadline = microtime(true) + 3.0;
while (!OxphpSessionWriteProbe::$writing && OxphpSessionWriteProbe::$written === null
    && microtime(true) < $deadline) {
    oxphp_usleep(10_000);
}
$peekSock = session_inner_send('/tests/fibers/fixture_session_peek_during_write.php', str_repeat('6b', 16));

$writer = session_inner_read($writerSock);
$peek = json_decode(session_inner_read($peekSock), true);

$t = new TestCase('session_write_keeps_the_worker', 'fibers');

$t->meta('writer_body', $writer);
$t->meta('peek', $peek);

$t->assertSame('the writer ran', $writer, 'left open');
$t->assertTrue('the peek answered with JSON', is_array($peek));
if (!is_array($peek)) {
    $t->done();
}

// Taken first, so it is also the premise: a peek that ran before the writer
// reached its write would see no session and prove nothing.
$t->assertSame(
    'the peek ran only after the writer\'s write had finished',
    $peek['written'] ?? null,
    'who|s:6:"writer";'
);
$t->assertFalse('and not in the middle of it', $peek['writing'] ?? true);
$t->assertSame('it found no session id standing', $peek['session_id'] ?? null, '');
$t->assertSame('and no $_SESSION of another client', $peek['who'] ?? null, 'none');

$t->done();
