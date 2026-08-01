<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('worker_mode_active', 'worker');

$t->assertTrue('oxphp_is_worker() === true', oxphp_is_worker());

$t->done();
