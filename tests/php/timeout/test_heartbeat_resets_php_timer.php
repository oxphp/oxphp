<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('heartbeat_resets_php_timer', 'timeout');

// Arm PHP's own max_execution_time at 1 second.
// Without oxphp_request_heartbeat() also resetting Zend's timer, the
// subsequent usleep(1.2s) would let the SIGPROF fire, and the next VM
// step would fatal with "Maximum execution time of 1 second exceeded".
set_time_limit(1);

// Heartbeat reschedules both deadlines to now + 10s:
//   - server-side REQUEST_TIMEOUT_SECONDS → prevents 408
//   - PHP's Zend timer                    → prevents "Maximum execution time exceeded"
oxphp_request_heartbeat(10);

// Sleep past the original 1-second PHP limit.
usleep(1_200_000);

// If we got here, both timers were extended correctly — no fatal fired,
// no 408 came back from the server. An unextended PHP timer would have
// terminated the script before this line executed.
$t->assertTrue('PHP max_execution_time extended past original 1s limit', true);
$t->done();
