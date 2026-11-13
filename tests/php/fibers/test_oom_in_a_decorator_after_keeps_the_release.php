<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/write_cancel_probe.php';

// A stream ended at a write whose client has gone, made from inside a decorated
// function. The worker closes the decorated call before it gives back what the
// abandoned frames were holding, which calls the decorator's after(), and
// after() runs out of memory. The engine reports that inside after() and is left
// noting that it did — a note under which it refuses every call through
// zend_call_function(). What the request held is given back after that: a
// stream with a userland filter, whose onClose() the engine is asked to call as
// the stream closes, and still calls.
//
// A stage of its own file for the reason
// fibers/test_write_cancel_in_a_fiber_releases_frames gives.

$t = new TestCase('oom_in_a_decorator_after_keeps_the_release', 'fibers');

if (write_cancel_stage($t, 'stream-decorated-oom', '?in=stream-decorated-oom')) {
    $reported = write_cancel_wait(static fn (): bool => OxphpWriteCancelProbe::$report !== null, 6.0);
    $report = OxphpWriteCancelProbe::$report ?? ['freed' => false];
    $t->meta('report', $report);

    $t->assertSame('the write it resumed into ended it', OxphpWriteCancelProbe::$stage, 'before-write');
    $t->assertTrue('the inner request reported from its shutdown function', $reported);
    $t->assertSame(
        "the decorator's after() ran and ran out of memory",
        OxphpWriteCancelProbe::$afterStage,
        'exhausting'
    );
    $t->assertTrue('what the request held was given back', $report['freed']);
    $t->assertTrue('and the filter on its stream was closed', OxphpWriteCancelProbe::$filterClosed);
}

$t->done();
