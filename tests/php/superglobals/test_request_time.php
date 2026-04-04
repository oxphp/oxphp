<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('request_time', 'superglobals');
$t->assertType('REQUEST_TIME is integer', $_SERVER['REQUEST_TIME'], 'integer');
$t->assertTrue('REQUEST_TIME is within 5s of now', abs(time() - $_SERVER['REQUEST_TIME']) <= 5);
$t->done();
