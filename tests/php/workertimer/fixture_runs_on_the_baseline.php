<?php

declare(strict_types=1);

// Inner request for workertimer/test_event_loop_request_has_its_own_deadline.
//
// Never calls set_time_limit(), so the only limit it has is the one every
// request starts with — two seconds in this profile — and spends about six
// seconds busy. Ended part-way if it was given that limit when it was taken;
// otherwise it finishes and says so.

require_once __DIR__ . '/timer_probe.php';

OxphpWorkerTimerProbe::start('baseline');

OxphpWorkerTimerProbe::burn(6.0);

OxphpWorkerTimerProbe::$runs['baseline']['finished'] = true;
echo 'finished';
