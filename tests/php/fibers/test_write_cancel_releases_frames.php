<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/write_cancel_probe.php';

// A request whose client leaves while it is parked runs on to the end of its
// handler, and the write it resumes into raises no error and ends nothing. The
// worker has to release what the request's frames were holding all the same: it
// keeps serving, and whatever it does not release it holds for the rest of its
// life — per request.
//
// The inner request parks holding an object, this one takes its client away and
// waits until the server has seen it go, and the inner request then resumes into
// a write. What it was holding is read back through a weak reference.
//
// A client leaving used to end the request at that write. It no longer does,
// because ending it there is a longjmp, a longjmp never re-enters the engine,
// and a `finally` is reached only from inside it — so userland's own cleanup
// was skipped on a worker that outlives the request it belonged to. See
// fibers/test_cancel_runs_userland_cleanup for what that stranded. The
// destructor below was never the gap: it ran either way, and what moved is who
// runs it. The reasons that still end a request where it stands — timeout,
// drain, a supervisor giving up — still take the unwinding path, as does a
// client leaving a request that is streaming, whose loop has no other bound.
// The worker still has to give its frames back on that path, and the two stream
// cases at the end of this file are what reach it.

$t = new TestCase('write_cancel_releases_frames', 'fibers');

// From the handler. The report comes from a shutdown function the worker runs
// after the cleanup.
if (write_cancel_stage($t, 'handler', '')) {
    $reported = write_cancel_wait(static fn (): bool => OxphpWriteCancelProbe::$report !== null, 6.0);
    $t->assertTrue('handler: the inner request reported from its shutdown function', $reported);

    $report = OxphpWriteCancelProbe::$report ?? ['stage' => null, 'freed' => false, 'last_error' => null];

    // Without these two the last assertion proves nothing: a request that never
    // reached its write, or that was ended by an error, took a different path out.
    $t->assertSame('handler: the inner request resumed into its write and ran past it', $report['stage'], 'after-write');
    $t->assertNull('handler: and that ending raised no error', $report['last_error']);

    $t->assertTrue('handler: what the abandoned frame was holding did not outlive its request', $report['freed']);
}

// From a shutdown function, after the handler had a fatal. The fatal stops the
// script before the reporting shutdown function is registered, so the release
// is read directly, for as long as the request could take to get there.
if (write_cancel_stage($t, 'shutdown', '?in=shutdown')) {
    $freed = write_cancel_wait(static fn (): bool => OxphpWriteCancelProbe::freed(), 6.0);

    // Reaching the far side of the write is what says the shutdown machinery
    // was left to finish: stopping between the two would mean it was not.
    $t->assertSame('shutdown: the shutdown function ran past the write it resumed into', OxphpWriteCancelProbe::$stage, 'after-write');

    $t->assertTrue('shutdown: what the abandoned frame was holding did not outlive its request', $freed);
}

// From the handler, holding an object whose destructor sleeps. The destructor
// has to run, and to run as part of the request rather than after it: an object
// released by the worker's own cleanup is one the request did not get to see
// out, and a destructor is userland code with the same claim on running as a
// `finally`.
if (write_cancel_stage($t, 'destructor', '?in=destructor')) {
    $destructed = write_cancel_wait(
        static fn (): bool => OxphpWriteCancelProbe::$stage === 'destructed',
        6.0
    );

    $t->assertTrue('destructor: what the frame was holding was destroyed', $destructed);
    // At the function's own return, which is where the request releases its own
    // frame — not by the worker's cleanup afterwards, which would see
    // 'before-write'.
    $t->assertSame('destructor: by the request, at the return that dropped it', OxphpWriteCancelProbe::$stageAtDestruct, 'after-write');
}

// A stream, which is still ended at the write it resumes into. The request does
// not get to its return, so what its frame held is given back by the worker.
if (write_cancel_stage($t, 'stream', '?in=stream')) {
    $freed = write_cancel_wait(static fn (): bool => OxphpWriteCancelProbe::freed(), 6.0);

    $t->assertSame('stream: the write it resumed into ended it', OxphpWriteCancelProbe::$stage, 'before-write');
    $t->assertTrue('stream: what the abandoned frame was holding did not outlive its request', $freed);
    // Which path it took. A stream is also ended when an interrupt finds its
    // client gone, and that path gives the frames back too, so the two above
    // pass on it as well; it is told apart by the fatal it reports, which the
    // write does not.
    $reported = write_cancel_wait(static fn (): bool => OxphpWriteCancelProbe::$report !== null, 6.0);
    $t->assertTrue('stream: the inner request reported from its shutdown function', $reported);
    $t->assertNull(
        'stream: ended by the write, not by an interrupt',
        (OxphpWriteCancelProbe::$report ?? ['last_error' => 'no report'])['last_error']
    );
}

// The same, holding an object whose destructor throws. The destructor runs
// during the give-back, and its throw ends that give-back where it stands — what
// is left stays allocated, as the changelog says — but nothing more: this
// request, on the same worker, is still here to read it, and so is the worker.
if (write_cancel_stage($t, 'stream-throw', '?in=stream-throw')) {
    $ran = write_cancel_wait(static fn (): bool => OxphpWriteCancelProbe::$stageAtDestruct !== null, 6.0);

    $t->assertTrue('stream-throw: its destructor ran', $ran);
    $t->assertSame(
        'stream-throw: after the write had ended the request',
        OxphpWriteCancelProbe::$stageAtDestruct,
        'before-write'
    );
}

$t->done();
