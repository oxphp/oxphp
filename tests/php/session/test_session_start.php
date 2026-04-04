<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('session_start', 'session');

session_start();

$sid = session_id();
$t->assertNotEmpty('session_id() is not empty', $sid);
$t->assertType('$_SESSION is array', $_SESSION, 'array');

$t->meta('session_id', $sid);

$t->done();
