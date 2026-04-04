<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('is_worker_false', 'sapi');

$t->assertFalse('oxphp_is_worker() === false', oxphp_is_worker());

$t->done();
