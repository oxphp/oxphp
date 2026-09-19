<?php

declare(strict_types=1);

// Read before anything else in this file: require_once compiles and runs code,
// and the whole question is what this request found standing when it started.
$sessionIsset = isset($_SESSION);
$sessionId = session_id();

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/session_fastpath_latch.php';

// Second half of the pair seeded by test_session_fastpath_seed, which ran a
// session on this worker and closed it properly.
//
// Closing a session marks it none and writes it back. It does not take the id
// off the thread, and it does not remove the _SESSION entry the session module
// put in the symbol table — both of those belong to the end of a request, which
// a worker does not run. So without a reset that removes them, this request
// starts able to read the previous one's array under its own name, and any
// session it starts of its own is adopted into the previous one's id instead of
// the one its cookie names.
//
// The latch is read before the assertions rather than assumed: with only the
// second half selected, or the two reordered, every assertion below would pass
// on a worker that had never run a session at all.

$seeded = SessionFastPathLatch::seededId();

$t = new TestCase('session_fastpath_leaves_nothing', 'fibers');

$t->assertSame(
    'the seeding request ran on this worker before this one',
    $seeded,
    str_repeat('fa', 16)
);

$t->assertFalse('$_SESSION does not survive the request that created it', $sessionIsset);
$t->assertSame('nor does the session id it closed', $sessionId, '');

$t->done();
