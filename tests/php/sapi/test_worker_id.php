<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('worker_id', 'sapi');

$wid = oxphp_worker_id();
$t->assertType('oxphp_worker_id() is integer', $wid, 'integer');
$t->assertTrue('oxphp_worker_id() >= 0', $wid >= 0);

$t->done();
