<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/session_flush_throw_probe.php';

// Follows fibers/test_session_written_after_flush_throw.

$t = new TestCase('session_written_after_flush_throw_probe', 'fibers');

// Without these the two below prove nothing: a callback that never ran, or
// that ran only after the session was written, throws nothing in its way.
$t->assertTrue('the output callback ran', OxphpSessionFlushThrowProbe::$callbackRan);
$t->assertTrue('before the session was written', OxphpSessionFlushThrowProbe::$callbackRanBeforeWrite === true);

$t->assertSame(
    'the save handler was written with the session',
    OxphpSessionFlushThrowProbe::$written,
    'marker|s:4:"kept";'
);
$t->assertTrue('and closed', OxphpSessionFlushThrowProbe::$closed);

$t->done();
