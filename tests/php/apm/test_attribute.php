<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('attribute', 'apm');

oxphp_apm_attribute('key', 'value');
$t->assertTrue('oxphp_apm_attribute completed without exception', true);

$t->done();
