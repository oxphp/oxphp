<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/write_cancel_probe.php';

// A stream whose client has gone is ended at the write it resumes into, and the
// worker gives back what its frames were holding. A destructor that runs then
// can start a Fiber of its own: the worker holds the thread for the request
// until that cleanup is done, but it does not stop the code in it from
// switching fibers, which a client built on fibers needs to close.

$t = new TestCase('write_cancel_release_lets_a_destructor_start_a_fiber', 'fibers');

if (write_cancel_stage($t, 'stream-fiber-starting-destructor', '?in=stream-fiber-starting-destructor')) {
    $returned = write_cancel_wait(
        static fn (): bool => OxphpWriteCancelProbe::$stage === 'destructor-returned',
        6.0
    );

    $t->assertSame(
        'the destructor ran in the give-back, after the write had ended the request',
        OxphpWriteCancelProbe::$stageAtDestruct,
        'before-write'
    );
    $t->assertTrue('the Fiber it started ran', OxphpWriteCancelProbe::$destructorFiberRan);
    $t->assertTrue('and the destructor got past the start', $returned);
}

$t->done();
