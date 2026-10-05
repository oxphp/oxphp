<?php

declare(strict_types=1);

require_once __DIR__ . '/timer_probe.php';

// Once a request's limit has ended it, what it runs next — its shutdown
// functions — runs under no limit, as it does under PHP itself, and that holds
// across a park: a shutdown function that waits is let finish, not ended again
// by the limit that has already been answered.
//
// ?action=trigger never calls set_time_limit() and spins until the profile's two
// seconds end it; its line expects the 504. A shutdown function then parks for
// a third of a second and records that it got past the park. ?action=check reads
// that.

if (($_GET['action'] ?? '') === 'trigger') {
    OxphpWorkerTimerProbe::reset();
    OxphpWorkerTimerProbe::start('cleanup');

    register_shutdown_function(static function (): void {
        $before = hrtime(true);
        // Hooked in this profile: parks the request, which comes back to the
        // worker through a resume.
        usleep(300_000);
        OxphpWorkerTimerProbe::$cleanupSlept = (hrtime(true) - $before) / 1e9;
    });

    $stop = microtime(true) + 5.0;
    while (microtime(true) < $stop) {
        // spin until max_execution_time ends the request
    }

    OxphpWorkerTimerProbe::$runs['cleanup']['finished'] = true;
    echo "unreachable: max_execution_time must end this request\n";
    return;
}

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('timed_out_request_may_park_in_its_shutdown_function', 'workertimer');

$run = OxphpWorkerTimerProbe::$runs['cleanup'] ?? null;
$t->assertNotNull('the trigger request ran', $run);

if ($run !== null) {
    $t->assertFalse('it did not run to its end', $run['finished']);
    $t->assertTrue('its limit ended it: the timeout bit is up', $run['timedOut']);
    $t->assertNotNull(
        'its shutdown function got past the park',
        OxphpWorkerTimerProbe::$cleanupSlept
    );
    if (OxphpWorkerTimerProbe::$cleanupSlept !== null) {
        $t->meta('cleanup_slept_seconds', round(OxphpWorkerTimerProbe::$cleanupSlept, 3));
        $t->assertGreaterThan(
            'having actually waited',
            OxphpWorkerTimerProbe::$cleanupSlept,
            0.25
        );
    }
}

$t->done();
