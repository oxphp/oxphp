<?php

declare(strict_types=1);

// Ticker for fibers/test_time_limit_is_not_restarted_by_a_park: runs beside the
// request under test on the same worker and counts each time it gets to run,
// until that request is over. It gets to run only while that request is parked.

require_once __DIR__ . '/time_limit_probe.php';

$until = microtime(true) + 8.0;
while (!OxphpTimeLimitProbe::$over && microtime(true) < $until) {
    // Hooked: parks this request until the timer is due.
    usleep(200);
    OxphpTimeLimitProbe::$ticks++;
}

echo 'ticked';
