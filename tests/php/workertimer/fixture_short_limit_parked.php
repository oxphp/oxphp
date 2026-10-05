<?php

declare(strict_types=1);

// Inner request for workertimer/test_deadline_ends_the_request_it_belongs_to:
// the one that runs out. Sets a one-second limit and parks for three seconds, so
// its limit runs out while it is parked and the worker is running the other
// inner request. It has to be ended as soon as it is resumed, before the
// statement after the sleep.

require_once __DIR__ . '/timer_probe.php';

OxphpWorkerTimerProbe::start('short');

set_time_limit(1);

// Hooked in this profile: parks this request for the whole three seconds.
usleep(3_000_000);

OxphpWorkerTimerProbe::$runs['short']['woke'] = true;
OxphpWorkerTimerProbe::$runs['short']['finished'] = true;
echo 'finished';
