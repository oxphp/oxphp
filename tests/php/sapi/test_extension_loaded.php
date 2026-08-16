<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('extension_loaded', 'sapi');

$t->assertTrue("extension_loaded('oxphp_sapi') === true", extension_loaded('oxphp_sapi') === true);

$t->done();
