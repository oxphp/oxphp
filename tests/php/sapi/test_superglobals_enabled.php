<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('superglobals_enabled', 'sapi');

$t->assertTrue('oxphp_superglobals_enabled() === true', oxphp_superglobals_enabled() === true);

$t->done();
