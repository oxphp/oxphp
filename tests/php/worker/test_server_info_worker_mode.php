<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('server_info_worker_mode', 'worker');

$info = oxphp_server_info();
$t->assertType('oxphp_server_info() returns array', $info, 'array');
$t->assertKeyExists('has key: worker_mode', $info, 'worker_mode');
$t->assertTrue('worker_mode is true', $info['worker_mode'] === true);

$t->done();
