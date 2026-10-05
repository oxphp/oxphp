<?php

declare(strict_types=1);

// Inner request for workertimer/test_unbounded_request_is_not_ended_by_a_neighbours_limit:
// the one with no limit, as a stream that calls set_time_limit(0) has. Switches
// its limit off before it registers, so the test sends the other request only
// once this one's limit is gone, then spends about three seconds busy. Nothing
// may end it: its own limit is off, and another request's is not its own.

require_once __DIR__ . '/timer_probe.php';

set_time_limit(0);

OxphpWorkerTimerProbe::start('unbounded');

OxphpWorkerTimerProbe::burn(3.0);

OxphpWorkerTimerProbe::$runs['unbounded']['finished'] = true;
echo 'finished';
