<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/timer_probe.php';

// A request whose limit is over while it is parked, and which a recycle of its
// worker then cancels where it waits, is ended by the cancellation, and all of
// its shutdown functions run.
//
// ?action=trigger sends the inner request — one second, then a five second
// park — waits until even the profile's two seconds would be over, so the inner
// request is overdue whether or not its own set_time_limit() took, and has the
// worker exit. The worker leaves its loop once this request is done, with the
// inner one still parked, and cancels it there. Its connection is kept open past
// this request, so it is the recycle that ends it and not its client going away.
//
// ?action=check runs on the worker that replaced it and reads what the inner
// request's shutdown functions wrote: both of them, and the second saw the
// cancellation as the request's last error and the connection aborted, with no
// timeout bit. A timeout delivered on the resume would be pending through the
// cancellation's bailout, which never reaches an opcode, and would end the first
// shutdown function at its first internal call instead.

$marker = '/tmp/oxphp-workertimer-cancelled.txt';

$t = new TestCase('overdue_request_cancelled_by_a_recycle_runs_its_shutdown_functions', 'workertimer');

if (($_GET['action'] ?? '') === 'trigger') {
    set_time_limit(20);
    OxphpWorkerTimerProbe::reset();
    file_put_contents($marker, "trigger\n");

    OxphpWorkerTimerProbe::$outerInFlight = true;
    OxphpWorkerTimerProbe::$heldOpen = OxphpWorkerTimerProbe::send('/tests/workertimer/fixture_cancelled_while_overdue.php');

    // Hooked in this profile: each wait parks this request and lets the worker
    // take the inner one.
    $giveUp = microtime(true) + 3.0;
    while (!isset(OxphpWorkerTimerProbe::$runs['overdue']) && microtime(true) < $giveUp) {
        usleep(10_000);
    }
    usleep(2_300_000);
    OxphpWorkerTimerProbe::$outerInFlight = false;

    $run = OxphpWorkerTimerProbe::$runs['overdue'] ?? null;
    $t->assertNotNull('the inner request started', $run);
    if ($run !== null) {
        $t->assertTrue('it was taken beside this one', $run['beside']);
        $t->assertTrue('its limit is over, its own or the profile\'s', hrtime(true) - $run['started'] > 2_000_000_000);
        $t->assertFalse('and it is still parked', $run['woke'] || $run['ended'] !== 0);
    }

    OxPHP\Server\Worker::current()->scheduleExit();
    $t->done();
}

$lines = file($marker, FILE_IGNORE_NEW_LINES) ?: [];
$t->assertSame('the trigger line wrote the marker', $lines[0] ?? null, 'trigger');
$t->assertSame('the first shutdown function ran', $lines[1] ?? null, 'first');
$t->assertNotNull('and so did the second', $lines[2] ?? null);

$second = json_decode($lines[2] ?? 'null', true);
if (is_array($second)) {
    $t->assertSame('the request was ended by the cancellation', $second['message'], 'Request cancelled (shutdown)');
    $t->assertTrue('its connection is marked aborted', ($second['status'] & CONNECTION_ABORTED) !== 0);
    $t->assertFalse('and not timed out: the spent limit was not delivered', ($second['status'] & CONNECTION_TIMEOUT) !== 0);
}

$t->done();
