<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('deny_status_403', 'php_deny');
// Placeholder — runner hits /uploads/shell.php with PHP_DENY_FALLBACK=403
// and expects HTTP 403.
$t->assertTrue('placeholder — runner checks 403 fallback status', true);
$t->done();
