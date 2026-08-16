<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('status', 'apm');

oxphp_apm_status(1); // 1 = Ok
$t->assertTrue('oxphp_apm_status(1) completed without exception', true);

$t->done();
