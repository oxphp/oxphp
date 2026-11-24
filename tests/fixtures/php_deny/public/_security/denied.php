<?php

declare(strict_types=1);

require_once __DIR__ . '/../tests/test_helper.php';

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

// The request methods carry the fallback's keys too: the pattern on its own,
// the denied path as PATH_INFO.
$req = oxphp_http_request();
$t->assertSame('deniedPattern()', $req->deniedPattern(), 'uploads/**');
$t->assertSame('pathInfo() === OXPHP_DENIED_PATH', $req->pathInfo(), $_SERVER['OXPHP_DENIED_PATH'] ?? null);
$t->assertSame('scriptName() === SCRIPT_NAME', $req->scriptName(), $_SERVER['SCRIPT_NAME'] ?? null);
$t->assertSame('scriptFilename() === SCRIPT_FILENAME', $req->scriptFilename(), $_SERVER['SCRIPT_FILENAME'] ?? null);
$t->assertSame(
    'scriptName() . pathInfo() === PHP_SELF',
    $req->scriptName() . ($req->pathInfo() ?? ''),
    $_SERVER['PHP_SELF'] ?? null
);
$t->done();
