<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('sapi_name', 'sapi');

$t->assertSame('php_sapi_name() is cli-server', php_sapi_name(), 'cli-server');

$t->done();
