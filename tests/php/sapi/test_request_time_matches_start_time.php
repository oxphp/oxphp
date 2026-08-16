<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('request_time_matches_start_time', 'sapi');

$info = oxphp_server_info();
$t->assertKeyExists('oxphp_server_info has request_time', $info, 'request_time');

$startTime = oxphp_http_request()->startTime(true);
$t->assertType('Request::startTime(true) is float', $startTime, 'double');

$t->assertTrue(
    'Request::startTime equals oxphp_server_info()[request_time]',
    (float)$info['request_time'] === (float)$startTime
);

$t->assertTrue(
    'request_time is non-zero during an active request',
    (float)$info['request_time'] > 0.0
);

$t->done();
