<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/write_cancel_probe.php';

// A stream whose client has gone is ended at the write it resumes into, and the
// worker gives back what its frames were holding. The destructors that run then
// are the request's end, and one that writes to the client that has gone does not
// end the give-back: what it writes is dropped like anything else a worker-mode
// request writes after its client left. So the destructor runs past its write,
// and the one due after it runs too.

$t = new TestCase('write_cancel_release_runs_past_a_destructor_that_writes', 'fibers');

if (write_cancel_stage($t, 'stream-writing-destructor', '?in=stream-writing-destructor')) {
    $ran = write_cancel_wait(static fn (): bool => OxphpWriteCancelProbe::$nextDestructed, 6.0);

    $t->assertSame('the write it resumed into ended it', OxphpWriteCancelProbe::$stage, 'before-write');
    $t->assertSame(
        'the destructor that writes ran in the give-back',
        OxphpWriteCancelProbe::$stageAtDestruct,
        'before-write'
    );
    $t->assertTrue('and ran past its write', OxphpWriteCancelProbe::$writerReturned);
    $t->assertTrue('and the destructor after it ran', $ran);
}

$t->done();
