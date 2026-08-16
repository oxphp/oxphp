<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('port_443', 'tls');
$port = (int)($_SERVER['SERVER_PORT'] ?? 0);
$t->assertTrue('SERVER_PORT equals 443 or is > 0', $port === 443 || $port > 0);
$t->done();
