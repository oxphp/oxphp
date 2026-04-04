<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('start_time', 'http_object');
$req = oxphp_http_request();
$startTime = $req->startTime(true);
$t->assertType('startTime(true) is float', $startTime, 'double');
$t->assertTrue('startTime(true) is within 5s of microtime(true)', abs(microtime(true) - $startTime) <= 5);
$t->done();
