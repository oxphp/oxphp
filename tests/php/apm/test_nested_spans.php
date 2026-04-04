<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('nested_spans', 'apm');

$outer = oxphp_apm_start('outer_span');
$t->assertType('outer span id is integer', $outer, 'integer');

$inner = oxphp_apm_start('inner_span');
$t->assertType('inner span id is integer', $inner, 'integer');

oxphp_apm_end($inner);
$t->assertTrue('inner span ended without exception', true);

oxphp_apm_end($outer);
$t->assertTrue('outer span ended without exception', true);

$t->done();
