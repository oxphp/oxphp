<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/write_cancel_probe.php';

// A stream ended at a write whose client has gone, holding an object whose
// decorated destructor lets go of a Fiber suspended in a try whose finally
// fatals. The worker gives back what the abandoned frame was holding, which runs
// that destructor; destroying the fiber inside it runs the finally, and the
// fatal there ends the destructor from inside that fiber, so the engine clears
// its record of the current fiber on the way out of it. The worker then closes
// the destructor's call, which the fatal left open, and that calls the
// decorator's after(): it runs in the request's fiber, and then writes to the
// same gone client from a Fiber it starts, which ends after() from inside that
// fiber and clears the record again. The request's shutdown function, after
// all of it, runs in the request's fiber too.
//
// A stage of its own file for the reason
// fibers/test_write_cancel_in_a_fiber_releases_frames gives.

$t = new TestCase('fatal_in_a_released_destructor_keeps_the_request_fiber_for_its_decorator', 'fibers');

if (write_cancel_stage($t, 'stream-decorated-destructor', '?in=stream-decorated-destructor')) {
    $reported = write_cancel_wait(static fn (): bool => OxphpWriteCancelProbe::$report !== null, 6.0);
    $report = OxphpWriteCancelProbe::$report ?? ['last_error' => null];
    $t->meta('report', $report);
    $t->meta('fiber_at_after', OxphpWriteCancelProbe::$fiberAtAfter);

    // Neither the request nor its destructor got past the point it was ended
    // at: the destructor's call was still open when the worker closed it.
    $t->assertSame('the write it resumed into ended it', OxphpWriteCancelProbe::$stage, 'before-write');
    $t->assertTrue('the inner request reported from its shutdown function', $reported);
    $t->assertContains(
        'the destructor destroyed the fiber, whose finally fataled',
        (string) $report['last_error'],
        'OxphpWriteCancelHeldFiberFinally'
    );
    $t->assertSame(
        "the decorator's after() ran, and its own write ended it",
        OxphpWriteCancelProbe::$afterStage,
        'writing'
    );
    $t->assertNotNull('the request ran in a fiber', OxphpWriteCancelProbe::$requestFiber);
    $t->assertSame(
        "the decorator's after() ran in that fiber",
        OxphpWriteCancelProbe::$fiberAtAfter,
        OxphpWriteCancelProbe::$requestFiber
    );
    $t->assertSame(
        'its shutdown function ran in that fiber',
        $report['fiber'] ?? null,
        OxphpWriteCancelProbe::$requestFiber
    );
}

$t->done();
