<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('boot_request_time_zero', 'worker');

// $bootInfo is captured by worker_entry.php at top level (boot phase,
// before oxphp_worker enters its receive loop) and passed into the
// worker closure via use($bootInfo). PHP include runs in the includer's
// scope so we read it directly here.
$t->assertType('bootInfo is array', $bootInfo ?? null, 'array');
$t->assertKeyExists('bootInfo has request_time', $bootInfo, 'request_time');
$t->assertKeyExists('bootInfo has request_start_time', $bootInfo, 'request_start_time');

$t->assertTrue(
    'boot-phase oxphp_server_info()[request_time] is 0.0',
    (float)$bootInfo['request_time'] === 0.0
);
$t->assertTrue(
    'boot-phase Request::startTime() is 0.0',
    (float)$bootInfo['request_start_time'] === 0.0
);

$t->done();
