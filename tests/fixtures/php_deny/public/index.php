<?php

declare(strict_types=1);

require_once __DIR__ . '/tests/test_helper.php';

$t = new TestCase('test_deny_framework_inert', 'php_deny');
$t->assertEqual(
    'REQUEST_URI',
    $_SERVER['REQUEST_URI'] ?? '',
    '/uploads/shell.php'
);
$t->assertTrue(
    'OXPHP_DENIED_PATH is unset',
    !isset($_SERVER['OXPHP_DENIED_PATH'])
);
$t->assertTrue(
    'OXPHP_DENIED_PATTERN is unset',
    !isset($_SERVER['OXPHP_DENIED_PATTERN'])
);
$t->done();
