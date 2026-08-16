<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// Initial budget: 1 second. set_time_limit(10) re-arms SIGALRM to 10s, so
// the 1.5-second sleep below — which would have busted the original budget —
// completes successfully. (Sleep stays under the 2s server wrapper.)
ini_set('max_execution_time', '1');
set_time_limit(10);
usleep(1_500_000);

$t = new TestCase('set_time_limit_extends', 'timeout');
$t->assertTrue('request survived past original 1s budget after set_time_limit(10)', true);
$t->done();
