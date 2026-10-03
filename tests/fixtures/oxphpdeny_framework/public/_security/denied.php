<?php

declare(strict_types=1);

require_once __DIR__ . '/../tests/test_helper.php';

http_response_code(404);

$t = new TestCase('oxphpdeny_fallback_script', 'oxphpdeny');
$t->assertSame(
    'OXPHP_DENIED_PATH',
    $_SERVER['OXPHP_DENIED_PATH'] ?? '',
    $_SERVER['HTTP_X_EXPECT_PATH'] ?? '(no X-Expect-Path)'
);
$t->assertSame(
    'OXPHP_DENIED_PATTERN',
    $_SERVER['OXPHP_DENIED_PATTERN'] ?? '',
    $_SERVER['HTTP_X_EXPECT_PATTERN'] ?? '(no X-Expect-Pattern)'
);
$t->assertSame('SCRIPT_NAME', $_SERVER['SCRIPT_NAME'] ?? '', '/_security/denied.php');
$t->done();
