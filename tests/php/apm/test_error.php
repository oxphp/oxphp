<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('error', 'apm');

oxphp_apm_error(new \Exception('test'));
$t->assertTrue('oxphp_apm_error completed without exception', true);

$t->done();
