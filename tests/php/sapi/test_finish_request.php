<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('finish_request', 'sapi');

$result = oxphp_finish_request();
$t->assertTrue('oxphp_finish_request() returns true', $result === true);

$t->done();

echo 'AFTER_FINISH';
