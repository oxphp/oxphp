<?php

declare(strict_types=1);

// Inner request for fibers/test_time_limit_is_not_restarted_by_a_park.
//
// Sets a one-second limit, then spends about three seconds busy in slices of
// fifty milliseconds with a park after each. The slices are busy loops so that
// the limit is reached after about twenty of them whichever clock it counts —
// wall time on a Linux ZTS build, CPU time elsewhere — unless something starts
// it again along the way, in which case the request finishes and says so.

require_once __DIR__ . '/time_limit_probe.php';

register_shutdown_function(static function (): void {
    OxphpTimeLimitProbe::$timedOut = (connection_status() & CONNECTION_TIMEOUT) !== 0;
    OxphpTimeLimitProbe::$over = true;
});

set_time_limit(1);

for ($slice = 0; $slice < 60; $slice++) {
    $until = microtime(true) + 0.05;
    while (microtime(true) < $until) {
        // burn
    }

    // Hooked in this profile: parks this request and hands the worker back.
    // Counted as a park only if the ticker ran meanwhile, which it can do only
    // while this request is parked — a sleep that did not park counts nothing.
    $ticks = OxphpTimeLimitProbe::$ticks;
    usleep(1000);
    if (OxphpTimeLimitProbe::$ticks !== $ticks) {
        OxphpTimeLimitProbe::$parks++;
    }
}

OxphpTimeLimitProbe::$finished = true;
echo 'finished';
