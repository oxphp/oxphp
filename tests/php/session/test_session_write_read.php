<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('session_write_read', 'session');

session_start();

$_SESSION['test_key'] = 'test_value';

$t->assertSame('written value readable in same request', $_SESSION['test_key'], 'test_value');
$t->assertKeyExists('test_key exists in $_SESSION', $_SESSION, 'test_key');

$t->done();
