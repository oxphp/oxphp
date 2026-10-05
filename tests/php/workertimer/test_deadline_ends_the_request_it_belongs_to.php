<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/timer_probe.php';

// A time limit that runs out ends the request it belongs to, and only that one.
//
// Two inner requests run side by side on this worker while this one waits on
// both. One sets ten seconds and is busy for four. The other sets one second and
// parks for three, so its second is over while the worker is running the first.
// The first has to finish untouched — no timeout bit, a normal answer — and the
// second has to be ended by its own limit, with the bit and a 504, without
// running the statement after its sleep. That the second's limit ran out while
// the first was alive is measured rather than assumed: the first has to have
// started before it and ended after it.

$t = new TestCase('deadline_ends_the_request_it_belongs_to', 'workertimer');

set_time_limit(20);

OxphpWorkerTimerProbe::reset();

OxphpWorkerTimerProbe::$outerInFlight = true;
$longSock = OxphpWorkerTimerProbe::send('/tests/workertimer/fixture_long_limit.php');
$shortSock = OxphpWorkerTimerProbe::send('/tests/workertimer/fixture_short_limit_parked.php');
[$longStatus] = OxphpWorkerTimerProbe::receive($longSock, 12.0);
[$shortStatus] = OxphpWorkerTimerProbe::receive($shortSock, 12.0);
OxphpWorkerTimerProbe::$outerInFlight = false;

$long = OxphpWorkerTimerProbe::$runs['long'] ?? null;
$short = OxphpWorkerTimerProbe::$runs['short'] ?? null;
$t->assertNotNull('the request with room ran', $long);
$t->assertNotNull('the request that runs out ran', $short);

if ($long !== null && $short !== null) {
    $t->assertTrue('both were taken beside this one', $long['beside'] && $short['beside']);

    $shortDeadline = $short['started'] + 1_000_000_000;
    $t->assertTrue(
        'the second limit ran out while the first request was alive',
        $long['started'] < $shortDeadline && $long['ended'] > $shortDeadline
    );

    $t->assertTrue('the request with room ran to its end', $long['finished']);
    $t->assertFalse('and was not marked as timed out', $long['timedOut']);
    $t->assertSame('and was answered normally', $longStatus, 200);

    $t->assertFalse('the request that ran out did not get past its sleep', $short['woke']);
    $t->assertTrue('it was ended by its own limit: the timeout bit is up', $short['timedOut']);
    $t->assertSame('and it was answered as a deadline', $shortStatus, 504);
    $t->assertSame(
        'the fatal names its own one second',
        $short['message'],
        'Maximum execution time of 1 second exceeded'
    );

    $t->meta('long_lived_seconds', round(OxphpWorkerTimerProbe::lived('long'), 3));
    $t->meta('short_lived_seconds', round(OxphpWorkerTimerProbe::lived('short'), 3));
}

$t->done();
