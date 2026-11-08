<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// $_SESSION['7'] is an integer key to the engine, and Session has to read it
// the way PHP code does.
$t = new TestCase('session_numeric_key', 'http_object');

session_start();
$_SESSION['7'] = 'seven';
$_SESSION['name'] = 'n';

$session = oxphp_http_request()->session();

$t->assertSame('get("7")', $session->get('7', '<DEFAULT>'), 'seven');
$t->assertTrue('has("7")', $session->has('7'));
$t->assertFalse('has("8")', $session->has('8'));
$t->assertSame('get("name")', $session->get('name', '<DEFAULT>'), 'n');

// The default session encoder warns about integer keys when it writes.
unset($_SESSION['7']);

$t->done();
