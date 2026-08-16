<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('server_info', 'sapi');

$info = oxphp_server_info();
$t->assertType('oxphp_server_info() is array', $info, 'array');
$t->assertKeyExists('has key: version', $info, 'version');
$t->assertKeyExists('has key: worker_id', $info, 'worker_id');
$t->assertKeyExists('has key: request_time', $info, 'request_time');
$t->assertKeyExists('has key: worker_mode', $info, 'worker_mode');

$t->done();
