<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/timer_probe.php';

// With max_execution_time at zero and max_input_time set, a request has no time
// limit, and a worker's request has none either. Request startup arms the
// thread's timer for max_input_time, and the start of the script sets the limit
// to zero without stopping that timer, so the worker's boot leaves it running:
// unless the worker stops it before serving, it ends whichever request is
// running when it runs out, with a fatal naming a limit of zero seconds.
//
// ?action=trigger is the worker's first request and runs PHP code without a
// break for a second longer than max_input_time. The worker booted on this
// thread before it took the request, so the timer its boot left runs out inside
// that stretch, however late after the server's start the boot came.
// ?action=check then reads what its shutdown function recorded, so a request
// that was ended reports how.

// Declared once: in worker mode both lines include this file on the same worker.
if (!function_exists('oxphp_workertimeroff_process_age')) {
    /** Seconds since the server process started: no fewer than since the worker booted. */
    function oxphp_workertimeroff_process_age(): float
    {
        $stat = (string) file_get_contents('/proc/self/stat');
        // Fields after the command name, which is in parentheses and may hold spaces.
        $fields = explode(' ', substr($stat, strrpos($stat, ')') + 2));
        $startedTicks = (int) $fields[19];
        $uptime = (float) explode(' ', (string) file_get_contents('/proc/uptime'))[0];

        // /proc counts in USER_HZ, which Linux fixes at 100.
        return $uptime - $startedTicks / 100;
    }
}

if (($_GET['action'] ?? '') === 'trigger') {
    // The probe first: its shutdown function has to run before the one TestCase
    // registers, which throws when it reports a fatal after output has started.
    OxphpWorkerTimerProbe::reset();
    OxphpWorkerTimerProbe::start('unlimited');
    $t = new TestCase('request_with_no_limit_is_not_ended_by_the_boot_timer', 'workertimeroff');

    $t->assertSame('premise: the profile switches the limit off', ini_get('max_execution_time'), '0');
    $t->assertSame('premise: and leaves max_input_time set', ini_get('max_input_time'), '12');

    // The process is older than the boot, so its age bounds how long the boot's
    // timer has run already.
    $age = oxphp_workertimeroff_process_age();
    $t->meta('process_age_at_start', round($age, 3));
    $t->assertLessThan('premise: this request started before the boot\'s timer would run out', $age, 11.0);

    // Counted from this request rather than from the process: the boot came
    // after the process started, by however long the server took to get to it,
    // and before this request.
    $stop = microtime(true) + 13.0;
    while (microtime(true) < $stop) {
        // run PHP code, where a time limit is able to end the request
    }

    OxphpWorkerTimerProbe::$runs['unlimited']['finished'] = true;
    $t->assertFalse('it was not ended: no timeout bit', (connection_status() & CONNECTION_TIMEOUT) !== 0);
    $t->done();
}

$t = new TestCase('request_with_no_limit_is_not_ended_by_the_boot_timer', 'workertimeroff');

$run = OxphpWorkerTimerProbe::$runs['unlimited'] ?? null;
$t->assertNotNull('the trigger request ran', $run);

if ($run !== null) {
    $t->assertSame('no time limit ended it', $run['message'], '');
    $t->assertFalse('its shutdown functions saw no timeout bit', $run['timedOut']);
    $t->assertTrue('and it ran to its end', $run['finished']);
}

$t->done();
