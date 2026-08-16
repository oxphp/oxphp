<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('scheme_https', 'tls');
$t->assertSame('$_SERVER[REQUEST_SCHEME] is "https"', $_SERVER['REQUEST_SCHEME'] ?? null, 'https');
$t->done();
