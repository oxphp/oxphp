<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/timer_probe.php';

// A request the worker takes while it has another in flight gets a time limit
// of its own.
//
// This request raises its own limit well past anything here and waits on an
// inner request, which the worker can then only take beside it, by the event
// loop. The inner one never sets a limit, so it starts with the profile's two
// seconds, and burns about six. It has to be ended about two seconds after it
// started: not much earlier, which would mean it was not given its limit but
// handed one already spent, and not much later, which would mean it ran on
// someone else's — this request's twenty seconds, or nothing at all.

$t = new TestCase('event_loop_request_has_its_own_deadline', 'workertimer');

set_time_limit(20);

OxphpWorkerTimerProbe::reset();

OxphpWorkerTimerProbe::$outerInFlight = true;
[$status, $body] = OxphpWorkerTimerProbe::receive(
    OxphpWorkerTimerProbe::send('/tests/workertimer/fixture_runs_on_the_baseline.php'),
    12.0
);
OxphpWorkerTimerProbe::$outerInFlight = false;

$run = OxphpWorkerTimerProbe::$runs['baseline'] ?? null;
$t->assertNotNull('the inner request ran', $run);

if ($run !== null) {
    $t->assertTrue(
        'it was taken while this request was in flight, so by the event loop',
        $run['beside']
    );
    $t->assertTrue('its shutdown functions ran', $run['ended'] > 0);
    $t->assertFalse('it did not run to its end', $run['finished']);
    $t->assertTrue(
        'its limit is what ended it: its shutdown functions saw the timeout bit',
        $run['timedOut']
    );
    $t->assertSame('and it was answered as a deadline', $status, 504);
    $t->assertSame(
        'the fatal names its own two seconds, not the twenty this request set',
        $run['message'],
        'Maximum execution time of 2 seconds exceeded'
    );

    $lived = OxphpWorkerTimerProbe::lived('baseline');
    $t->meta('inner_lived_seconds', round($lived, 3));
    $t->assertGreaterThan('it was given the whole of its two seconds', $lived, 1.5);
    $t->assertLessThan('and those two seconds ended it', $lived, 3.5);
}

$t->done();
