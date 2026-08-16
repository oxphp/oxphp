<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('request_time_float', 'superglobals');
$t->assertType('REQUEST_TIME_FLOAT is double', $_SERVER['REQUEST_TIME_FLOAT'], 'double');
$t->assertTrue('REQUEST_TIME_FLOAT is within 5s of now', abs(microtime(true) - $_SERVER['REQUEST_TIME_FLOAT']) <= 5);
$t->done();
