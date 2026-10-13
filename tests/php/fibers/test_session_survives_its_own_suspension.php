<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// The other side of fibers/session_does_not_outlive_its_request.
//
// Session state is given back as the request that holds it ends — and only
// then. A request that parks with its session open is still holding it, and
// must come back to all of it: its id, its $_SESSION and the session still
// active, so the write its end makes goes to the store under its own id.
//
// What a request that arrives in that window gets is the subject of
// fibers/session_arrival_waits_for_an_open_session.

$mine = str_repeat('e5', 16);

// PHP 8.6 turns session.use_strict_mode on by default, and strict mode swaps an
// id the store has never seen for a fresh one. The id here is named by the test
// itself, which reads it back, so it has to be kept.
ini_set('session.use_strict_mode', '0');

session_id($mine);
session_start();
$_SESSION['who'] = 'OUTER';

oxphp_sleep(0.05);

$idAfter = session_id();
$whoAfter = $_SESSION['who'] ?? 'gone';
$statusAfter = session_status();

$t = new TestCase('session_survives_its_own_suspension', 'fibers');

$t->assertSame('this request resumes still holding its own session id', $idAfter, $mine);
$t->assertSame('and its own data behind it', $whoAfter, 'OUTER');
$t->assertSame('with the session still active', $statusAfter, PHP_SESSION_ACTIVE);

session_write_close();

$t->done();
