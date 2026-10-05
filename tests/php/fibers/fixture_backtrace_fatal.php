<?php

declare(strict_types=1);

// Inner self-request for fibers/test_fatal_backtrace_stays_with_its_request.
//
// Requested twice by that test, and fatal both times, inside a function given
// the request's secret and an object:
//
//   - with park=1, it first registers a shutdown function that reads the
//     backtrace error_get_last() reports, parks in a hooked sleep, and reads it
//     again — so the request's own backtrace has to be with it while it is
//     parked, and come back with it;
//   - without, it is the request the worker serves in that window, and leaves
//     where the test can reach it a weak reference to the object its fatal was
//     given.

require_once __DIR__ . '/backtrace_probe.php';

// The error and exception handlers a worker runs with are whatever the last
// request to set them installed — the outer test's, here. Cleared first, so
// what this request raises is the fatal the test is about.
set_error_handler(null);
set_exception_handler(null);

header('Content-Type: text/plain');

$secret = (string) ($_GET['secret'] ?? '?');
$park = ($_GET['park'] ?? '0') === '1';

if ($park) {
    // A directive that travels with the request and warns each time it is
    // applied: '60x' is read as 60, with a warning about the unit. The resume
    // below applies it again, and any warning raised on the way back in is one
    // the engine lets go of the last fatal's backtrace for.
    @ini_set('default_socket_timeout', '60x');

    register_shutdown_function(static function (): void {
        $before = OxphpBacktraceProbe::traceSays(error_get_last());

        OxphpBacktraceProbe::$parked = true;
        // Hooked: parks this request's fiber, which frees the worker to serve
        // the second request while this one still has its fatal to report.
        sleep(1);
        OxphpBacktraceProbe::$parked = false;

        $after = OxphpBacktraceProbe::traceSays(error_get_last());

        echo "TRACE-BEFORE-PARK:$before\n";
        echo "TRACE-AFTER-PARK:$after\n";
        echo 'SOCKET-TIMEOUT-AFTER-PARK:' . ini_get('default_socket_timeout') . "\n";
    });
} else {
    echo 'PEER-PARKED:' . (int) OxphpBacktraceProbe::$parked . "\n";
}

$held = new \stdClass();
if (!$park) {
    OxphpBacktraceProbe::$held = \WeakReference::create($held);
}

echo "ARMED:$secret\n";
OxphpBacktraceProbe::fatal($secret, $held);
echo "NOT-REACHED\n";
