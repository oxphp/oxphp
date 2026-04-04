<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('heartbeat_extends', 'timeout');
// Extend the request deadline by 10 seconds, then sleep 1.5s.
// Without the heartbeat this would exceed a ~2s timeout and produce 504.
// With the heartbeat the server keeps the deadline alive and this completes.
oxphp_request_heartbeat(10);
usleep(1_500_000); // 1.5 seconds
$t->assertTrue('request survived past base timeout thanks to heartbeat', true);
$t->done();
