<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/session_fastpath_latch.php';

// First half of the pair; test_session_fastpath_leaves_nothing is the assertion.
// This request does everything right — it starts a session and writes it back
// before returning — and the next request on this worker must still find nothing
// of it.
//
// The two are separate requests rather than one because what is being checked is
// what crosses the boundary between them, and a request cannot observe its own
// teardown. They run back to back with nothing suspended, so the worker takes
// them on its blocking path: the one path whose reset does clear the session, and
// which still left the _SESSION entry standing in the symbol table because only
// the PS() slot was given back.

// PHP 8.6 turns session.use_strict_mode on by default, and strict mode swaps an
// id the store has never seen for a fresh one. The id here is named by the test
// itself, which reads it back, so it has to be kept.
ini_set('session.use_strict_mode', '0');

session_id(str_repeat('fa', 16));
session_start();
$_SESSION['who'] = 'FASTPATH';
session_write_close();

SessionFastPathLatch::seeded(session_id());

$t = new TestCase('session_fastpath_seed', 'fibers');

$t->assertSame(
    'the seeding request ran its session under the id it asked for',
    session_id(),
    str_repeat('fa', 16)
);

$t->done();
