<?php

declare(strict_types=1);

// Inner request for workertimer/test_deadline_ends_the_request_it_belongs_to:
// the one with room. Sets a ten-second limit and spends about four seconds busy,
// parking between slices, while the other inner request's one-second limit runs
// out. It has to finish.

require_once __DIR__ . '/timer_probe.php';

OxphpWorkerTimerProbe::start('long');

set_time_limit(10);

OxphpWorkerTimerProbe::burn(4.0);

OxphpWorkerTimerProbe::$runs['long']['finished'] = true;
echo 'finished';
