<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/write_cancel_probe.php';

// A stream ended at a write whose client has gone, holding a Fiber suspended in
// a try whose finally fatals. The worker gives back what the abandoned frame was
// holding, and destroying that fiber resumes it to run the finally: the fatal
// there is raised from inside a fiber the give-back resumed, so the engine
// clears its record of the current fiber on the way out of it. The request's
// shutdown functions run after the give-back, and in the request's fiber.
//
// A stage of its own file for the reason
// fibers/test_write_cancel_in_a_fiber_releases_frames gives.

$t = new TestCase('fatal_while_releasing_a_held_fiber_keeps_the_request_fiber', 'fibers');

if (write_cancel_stage($t, 'stream-held-fiber', '?in=stream-held-fiber')) {
    $reported = write_cancel_wait(static fn (): bool => OxphpWriteCancelProbe::$report !== null, 6.0);
    $report = OxphpWriteCancelProbe::$report ?? ['last_error' => null, 'fiber' => null];
    $t->meta('report', $report);

    $t->assertSame('the write it resumed into ended it', OxphpWriteCancelProbe::$stage, 'before-write');
    $t->assertTrue('the inner request reported from its shutdown function', $reported);
    // Only destroying the held fiber runs its finally, and only the give-back
    // destroys it: the request ended before its frame could.
    $t->assertContains(
        'the give-back destroyed the held fiber, whose finally fataled',
        (string) $report['last_error'],
        'OxphpWriteCancelHeldFiberFinally'
    );
    $t->assertNotNull('the request ran in a fiber', OxphpWriteCancelProbe::$requestFiber);
    $t->assertSame(
        'its shutdown function ran in that fiber',
        $report['fiber'],
        OxphpWriteCancelProbe::$requestFiber
    );
}

$t->done();
