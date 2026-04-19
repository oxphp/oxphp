<?php

declare(strict_types=1);

require __DIR__ . '/../tests/test_helper.php';

http_response_code(404);

$t = new TestCase('test_deny_script_fallback', 'php_deny');
$t->assertEqual(
    'OXPHP_DENIED_PATH',
    $_SERVER['OXPHP_DENIED_PATH'] ?? '',
    '/uploads/shell.php'
);
$t->assertEqual(
    'OXPHP_DENIED_PATTERN',
    $_SERVER['OXPHP_DENIED_PATTERN'] ?? '',
    'uploads/**'
);
$t->assertEqual(
    'SCRIPT_NAME',
    $_SERVER['SCRIPT_NAME'] ?? '',
    '/_security/denied.php'
);
$t->done();
