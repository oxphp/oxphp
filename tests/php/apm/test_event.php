<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('event', 'apm');

oxphp_apm_event('test_event', ['k' => 'v']);
$t->assertTrue('oxphp_apm_event completed without exception', true);

$t->done();
