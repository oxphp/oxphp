<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('session_destroy', 'session');

session_start();
$_SESSION['foo'] = 'bar';
$t->assertKeyExists('foo set before destroy', $_SESSION, 'foo');

session_unset();
session_destroy();

$t->assertSame('$_SESSION is empty after destroy', $_SESSION, []);

$t->done();
