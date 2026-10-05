<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/timer_probe.php';

// A request with no limit — a stream that has called set_time_limit(0) — is not
// ended by the limit of a request running beside it.
//
// Neither this request nor the first inner one has a limit. The first switches
// its own off and is then busy for about three seconds. Only once it has done
// so does this request send the second, which sets one second and parks for
// three. The second's limit runs out while the first is busy, and nothing else
// on the worker has a limit then — so if the second's limit were still counting
// on the thread while the second is parked, it would end the first. The first
// has to finish untouched; the second has to be ended by its own limit when it
// resumes.

$t = new TestCase('unbounded_request_is_not_ended_by_a_neighbours_limit', 'workertimer');

set_time_limit(0);

OxphpWorkerTimerProbe::reset();

OxphpWorkerTimerProbe::$outerInFlight = true;
$unboundedSock = OxphpWorkerTimerProbe::send('/tests/workertimer/fixture_unbounded.php');

$waitUntil = microtime(true) + 5.0;
while (!isset(OxphpWorkerTimerProbe::$runs['unbounded']) && microtime(true) < $waitUntil) {
    // Hooked in this profile: parks this request so the worker takes the first.
    usleep(10_000);
}
$t->assertTrue(
    'the request with no limit started before the second was sent',
    isset(OxphpWorkerTimerProbe::$runs['unbounded'])
);

$shortSock = OxphpWorkerTimerProbe::send('/tests/workertimer/fixture_short_limit_parked.php');
[$unboundedStatus] = OxphpWorkerTimerProbe::receive($unboundedSock, 12.0);
[$shortStatus] = OxphpWorkerTimerProbe::receive($shortSock, 12.0);
OxphpWorkerTimerProbe::$outerInFlight = false;

$unbounded = OxphpWorkerTimerProbe::$runs['unbounded'] ?? null;
$short = OxphpWorkerTimerProbe::$runs['short'] ?? null;
$t->assertNotNull('the request with no limit ran', $unbounded);
$t->assertNotNull('the request that runs out ran', $short);

if ($unbounded !== null && $short !== null) {
    $t->assertTrue('both were taken beside this one', $unbounded['beside'] && $short['beside']);

    $shortDeadline = $short['started'] + 1_000_000_000;
    $t->assertTrue(
        'the second limit ran out while the request with no limit was busy',
        $unbounded['started'] < $short['started'] && $unbounded['ended'] > $shortDeadline
    );

    $t->assertTrue('the request with no limit ran to its end', $unbounded['finished']);
    $t->assertFalse('and was not marked as timed out', $unbounded['timedOut']);
    $t->assertSame('and was answered normally', $unboundedStatus, 200);

    $t->assertTrue('the request that ran out was ended by its own limit', $short['timedOut']);
    $t->assertSame('and answered as a deadline', $shortStatus, 504);
    $t->assertSame(
        'the fatal names its own one second',
        $short['message'],
        'Maximum execution time of 1 second exceeded'
    );

    $t->meta('unbounded_lived_seconds', round(OxphpWorkerTimerProbe::lived('unbounded'), 3));
    $t->meta('short_lived_seconds', round(OxphpWorkerTimerProbe::lived('short'), 3));
}

$t->done();
