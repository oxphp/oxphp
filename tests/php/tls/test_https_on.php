<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('https_on', 'tls');
$t->assertSame('$_SERVER[HTTPS] is "on"', $_SERVER['HTTPS'] ?? null, 'on');
$t->done();
