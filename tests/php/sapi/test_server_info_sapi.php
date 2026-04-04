<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('server_info_sapi', 'sapi');

$info = oxphp_server_info();
$t->assertSame("oxphp_server_info()['sapi'] === 'oxphp'", $info['sapi'], 'oxphp');

$t->done();
