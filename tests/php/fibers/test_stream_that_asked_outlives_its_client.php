<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/write_cancel_probe.php';

// A stream that called ignore_user_abort(true) outlives its client, and the
// call reaches no other stream.
//
// A streaming request whose client leaves is ended at its next write or flush
// (see fibers/test_stream_cancel_still_ends_the_request): its loop is bounded by
// that client and nothing else. ignore_user_abort(true) is how a script says it
// has a bound of its own and wants to keep running — the same contract as under
// any other SAPI — so a stream that made the call has to get past the write.
// Both ways: with its headers still unsent at the park, where the write would
// end it, and with them sent, where the flush would interrupt it.
//
// The second half is the reason the first could not simply be granted. The flag
// is an ini directive, and ini values live on the worker thread. If the call
// outlived the request that made it, every stream the worker ran afterwards
// would be one that had asked, none of them would be ended when their client
// left, and a single call anywhere would leave the worker's thread held by
// streams nobody reads. So the same stage runs again, on the same worker,
// straight after, with a stream that did not ask — and that one must end at the
// write as before.
//
// Both legs read both edges: the request reaches the write, and one gets past it
// while the other does not.

$t = new TestCase('stream_that_asked_outlives_its_client', 'fibers');

if (write_cancel_stage($t, 'asked', '', 'fixture_stream_ignores_abort.php')) {
    $finished = write_cancel_wait(
        static fn (): bool => OxphpWriteCancelProbe::$stage === 'after-write',
        4.0
    );

    $t->assertTrue('asked: the stream got past the write its client was gone for', $finished);
}

if (write_cancel_stage($t, 'asked after headers', '?open=1', 'fixture_stream_ignores_abort.php')) {
    $finished = write_cancel_wait(
        static fn (): bool => OxphpWriteCancelProbe::$stage === 'after-write',
        4.0
    );

    $t->assertTrue('asked after headers: the stream got past the flush its client was gone for', $finished);
}

if (write_cancel_stage($t, 'did not ask', '', 'fixture_stream_cancel_ends_it.php')) {
    $ranPast = write_cancel_wait(
        static fn (): bool => OxphpWriteCancelProbe::$stage === 'after-write',
        4.0
    );

    $t->assertFalse('did not ask: the next stream on this worker was still ended at its write', $ranPast);
    $t->assertSame('did not ask: which is as far as it got', OxphpWriteCancelProbe::$stage, 'before-write');
}

$t->done();
