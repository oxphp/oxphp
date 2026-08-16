<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('start_end', 'apm');

$id = oxphp_apm_start('test');
$t->assertType('oxphp_apm_start returns integer', $id, 'integer');

oxphp_apm_end($id);
$t->assertTrue('oxphp_apm_end completed without exception', true);

$t->done();
