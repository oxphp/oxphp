<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('heartbeat_respects_disabled_php_timer', 'timeout');

// set_time_limit(0) disarms PHP's execution timer entirely — EG(timeout_seconds)
// drops to 0 and no SIGPROF is scheduled.
set_time_limit(0);

// Smoke test: oxphp_request_heartbeat() must not throw, warn, or fatal
// when PHP's timer has been disabled. The server-side bridge deadline is
// still extended so REQUEST_TIMEOUT_SECONDS=2 doesn't cut this request.
$result = oxphp_request_heartbeat(10);
$t->assertTrue('heartbeat returns true with disabled PHP timer', $result);

// Sleep past the original compose-profile baseline to prove the server-side
// deadline extension still works even when the PHP timer is off.
usleep(1_500_000);

$t->assertTrue('request survived past profile baseline with timer disabled', true);
$t->done();
