<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// set_time_limit(0) disarms the timer entirely (PHP CLI parity), so the
// 1.5-second sleep that would have busted the initial 1s budget completes.
// (Sleep stays under the 2s server wrapper.)
ini_set('max_execution_time', '1');
set_time_limit(0);
usleep(1_500_000);

$t = new TestCase('set_time_limit_zero_unlimited', 'timeout');
$t->assertTrue('request survived past original 1s budget after set_time_limit(0)', true);
$t->done();
