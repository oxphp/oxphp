<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/write_cancel_probe.php';

// A stream ended at a write whose client has gone, made from inside a Fiber
// the request started within a decorated function. Ending the request from
// inside that fiber clears the engine's record of the current fiber. The worker
// closes the decorated call before it gives back what the abandoned frames were
// holding, which calls the decorator's after(): it runs in the request's fiber,
// and then writes to the same gone client from a Fiber it starts, which ends
// after() from inside that fiber and clears the record again. The object the
// request held is given back after that, and its destructor runs in the
// request's fiber too.
//
// A stage of its own file for the reason
// fibers/test_write_cancel_in_a_fiber_releases_frames gives.

$t = new TestCase('write_cancel_in_a_decorator_keeps_the_request_fiber_for_the_release', 'fibers');

if (write_cancel_stage($t, 'stream-decorated', '?in=stream-decorated')) {
    $reported = write_cancel_wait(static fn (): bool => OxphpWriteCancelProbe::$report !== null, 6.0);
    $report = OxphpWriteCancelProbe::$report ?? ['freed' => false];
    $t->meta('report', $report);
    $t->meta('fiber_at_after', OxphpWriteCancelProbe::$fiberAtAfter);
    $t->meta('fiber_at_destruct', OxphpWriteCancelProbe::$fiberAtDestruct);

    $t->assertSame('the write it resumed into ended it', OxphpWriteCancelProbe::$stage, 'before-write');
    $t->assertTrue('the inner request reported from its shutdown function', $reported);
    $t->assertSame(
        "the decorator's after() ran, and its own write ended it",
        OxphpWriteCancelProbe::$afterStage,
        'writing'
    );
    $t->assertTrue('what the request held was given back', $report['freed']);
    $t->assertNotNull('the request ran in a fiber', OxphpWriteCancelProbe::$requestFiber);
    $t->assertSame(
        "the decorator's after() ran in that fiber",
        OxphpWriteCancelProbe::$fiberAtAfter,
        OxphpWriteCancelProbe::$requestFiber
    );
    $t->assertSame(
        'the destructor of what it held ran in that fiber',
        OxphpWriteCancelProbe::$fiberAtDestruct,
        OxphpWriteCancelProbe::$requestFiber
    );
}

$t->done();
