<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/write_cancel_probe.php';

// The stream stage of fibers/test_write_cancel_releases_frames, with the write
// made from inside a Fiber the request starts once it has resumed. A stream
// whose client has gone is still ended at the write; here the engine catches
// that ending inside the fiber first, destroys the fiber's frames, and then ends
// the request with it. The frame the write was made from is gone by the time the
// worker gives anything back, so what it can still give back is the request's
// own frame below the fiber — and that is where the object is held.
//
// A stage of its own file rather than of that one, whose stages already take
// most of what the runner allows a single request.

$t = new TestCase('write_cancel_in_a_fiber_releases_frames', 'fibers');

if (write_cancel_stage($t, 'stream-fiber', '?in=stream-fiber')) {
    $freed = write_cancel_wait(static fn (): bool => OxphpWriteCancelProbe::freed(), 6.0);

    $t->assertSame('the write it resumed into ended it', OxphpWriteCancelProbe::$stage, 'before-write');
    $t->assertTrue('what the frame below the fiber was holding did not outlive its request', $freed);
    // Ended by the write and not by an interrupt, which ends a stream whose
    // client has gone too and gives its frames back as well — told apart, as in
    // the stream stage of that file, by the fatal an interrupt reports and the
    // write does not.
    $reported = write_cancel_wait(static fn (): bool => OxphpWriteCancelProbe::$report !== null, 6.0);
    $t->assertTrue('the inner request reported from its shutdown function', $reported);
    $t->assertNull(
        'ended by the write, not by an interrupt',
        (OxphpWriteCancelProbe::$report ?? ['last_error' => 'no report'])['last_error']
    );
}

$t->done();
