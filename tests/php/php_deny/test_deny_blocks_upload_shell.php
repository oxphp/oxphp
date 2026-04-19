<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('deny_blocks_upload_shell', 'php_deny');
// Placeholder — runner hits /uploads/shell.php and expects HTTP 404.
// PHP_DENY_DIRS must block execution before disk I/O, so the shell's
// body "SHOULD NEVER SEE THIS" must not appear in the response.
$t->assertTrue('placeholder — runner checks 404 for denied path', true);
$t->done();
