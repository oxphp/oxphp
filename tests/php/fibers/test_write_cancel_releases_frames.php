<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/write_cancel_probe.php';

// A request whose client leaves while it is parked is ended by its next write,
// and that ending raises no error. The worker has to release what the request's
// frames were holding all the same: it keeps serving, and whatever it does not
// release it holds for the rest of its life — per abandoned request.
//
// The inner request parks holding an object, this one takes its client away and
// waits until the server has seen it go, and the inner request then resumes into
// a write. What it was holding is read back through a weak reference.

$t = new TestCase('write_cancel_releases_frames', 'fibers');

// From the handler. The report comes from a shutdown function the worker runs
// after the cleanup.
if (write_cancel_stage($t, 'handler', '')) {
    $reported = write_cancel_wait(static fn (): bool => OxphpWriteCancelProbe::$report !== null, 6.0);
    $t->assertTrue('handler: the inner request reported from its shutdown function', $reported);

    $report = OxphpWriteCancelProbe::$report ?? ['stage' => null, 'freed' => false, 'last_error' => null];

    // Without these two the last assertion proves nothing: a request that was
    // not ended at the write, or was ended by an error, took a different path out.
    $t->assertSame('handler: the inner request was ended by the write it resumed into', $report['stage'], 'before-write');
    $t->assertNull('handler: and that ending raised no error', $report['last_error']);

    $t->assertTrue('handler: what the abandoned frame was holding did not outlive its request', $report['freed']);
}

// From a shutdown function, after the handler had a fatal. The write ends the
// shutdown machinery and no later shutdown function runs, so the release is
// read directly, for as long as the request could take to get there.
if (write_cancel_stage($t, 'shutdown', '?in=shutdown')) {
    $freed = write_cancel_wait(static fn (): bool => OxphpWriteCancelProbe::freed(), 6.0);

    // Nothing between setting the stage and the write is a point the interrupt
    // flag is acted on, so stopping between the two means the write ended it.
    $t->assertSame('shutdown: the shutdown function was ended by the write it resumed into', OxphpWriteCancelProbe::$stage, 'before-write');

    $t->assertTrue('shutdown: what the abandoned frame was holding did not outlive its request', $freed);
}

// From the handler, holding an object whose destructor sleeps. The cleanup runs
// that destructor, and it must not park there: until the cleanup is through, the
// worker's shutdown state is still raised, and any request that ran in the
// meantime would take it for its own. This request is the one that would run,
// so it watches for the destructor's middle.
if (write_cancel_stage($t, 'destructor', '?in=destructor')) {
    $ranDuring = false;
    $destructed = write_cancel_wait(static function () use (&$ranDuring): bool {
        if (OxphpWriteCancelProbe::$stage === 'destructing') {
            $ranDuring = true;
        }

        return OxphpWriteCancelProbe::$stage === 'destructed';
    }, 6.0);

    $t->assertTrue('destructor: what the abandoned frame was holding was destroyed', $destructed);
    // Destroyed after the write and not at the function's own return, which
    // would see 'after-write'.
    $t->assertSame('destructor: by the cleanup after the write ended the request', OxphpWriteCancelProbe::$stageAtDestruct, 'before-write');
    $t->assertFalse('destructor: nothing else ran on the worker while it did', $ranDuring);
}

$t->done();
