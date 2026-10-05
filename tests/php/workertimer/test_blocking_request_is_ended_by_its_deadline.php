<?php

declare(strict_types=1);

require_once __DIR__ . '/timer_probe.php';

// The other edge: a request the worker picks up with nothing else running is
// ended by the limit it starts with, as it always has been.
//
// ?action=trigger never calls set_time_limit() and spins for five seconds, so
// the profile's two seconds end it; its line expects the 504. ?action=check
// then reads what its shutdown function recorded. The spin's own bound is
// shorter than the runner's 15 s request timeout, so a limit that stopped firing
// fails one line rather than holding the worker past the runner's patience.

if (($_GET['action'] ?? '') === 'trigger') {
    OxphpWorkerTimerProbe::reset();
    OxphpWorkerTimerProbe::start('blocking');

    $stop = microtime(true) + 5.0;
    while (microtime(true) < $stop) {
        // spin until max_execution_time ends the request
    }

    OxphpWorkerTimerProbe::$runs['blocking']['finished'] = true;
    echo "unreachable: max_execution_time must end this request\n";
    return;
}

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('blocking_request_is_ended_by_its_deadline', 'workertimer');

$run = OxphpWorkerTimerProbe::$runs['blocking'] ?? null;
$t->assertNotNull('the trigger request ran', $run);

if ($run !== null) {
    $t->assertFalse('it did not run to its end', $run['finished']);
    $t->assertTrue('its shutdown functions saw the timeout bit', $run['timedOut']);
    $t->assertSame(
        'and the fatal names the limit it started with',
        $run['message'],
        'Maximum execution time of 2 seconds exceeded'
    );

    $lived = OxphpWorkerTimerProbe::lived('blocking');
    $t->meta('lived_seconds', round($lived, 3));
    $t->assertGreaterThan('it was given the whole of its two seconds', $lived, 1.5);
    $t->assertLessThan('and those two seconds ended it', $lived, 3.5);
}

$t->done();
