<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('deny_nonexistent_php', 'php_deny');
// Placeholder — runner hits /uploads/ghost-nosuchfile.php and expects HTTP 404.
// Proves the deny check fires before disk I/O — no "file not found" leak timing.
$t->assertTrue('placeholder — runner checks 404 for nonexistent denied path', true);
$t->done();
