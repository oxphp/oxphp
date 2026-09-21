<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/write_cancel_probe.php';

// A streaming request whose client leaves is still ended at its next write.
//
// Its neighbour, fibers/test_cancel_skips_userland_cleanup, pins the opposite
// rule for an ordinary request: a client that goes away no longer ends it,
// because ending it is a longjmp that leaves userland's `finally` unrun on a
// worker that outlives the request. Letting the request finish is safe there
// because it was going to finish anyway.
//
// A stream was not going to finish. Its loop runs until the client it writes to
// goes away, and that is its only bound — the same grace applied here does not
// delay an ending, it removes the only one there is, and the worker thread is
// held for the life of the process. So this shape keeps the ending it has
// always had, which is also the contract the streaming documentation states:
// check connection_aborted() and return, or accept an ending that will not run
// your `finally`.
//
// Both edges are read. The request has to reach the write, or a run in which it
// never resumed out of its park would pass without testing anything; and it
// must not reach the far side of it.

$t = new TestCase('stream_cancel_still_ends_the_request', 'fibers');

if (write_cancel_stage($t, 'stream', '', 'fixture_stream_cancel_ends_it.php')) {
    // The staging spends the worker's interrupt flag before returning, so the
    // write is what has to end this request rather than the flag being acted on
    // when it resumes. Waiting for the far side of the write and not getting
    // there is the assertion; the wait is what gives it time to get there.
    $ranPast = write_cancel_wait(
        static fn (): bool => OxphpWriteCancelProbe::$stage === 'after-write',
        4.0
    );

    $t->assertFalse('stream: the write it resumed into ended the request', $ranPast);
    $t->assertSame('stream: which is as far as it got', OxphpWriteCancelProbe::$stage, 'before-write');
}

$t->done();
