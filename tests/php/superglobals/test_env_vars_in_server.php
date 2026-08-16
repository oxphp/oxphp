<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('env_vars_in_server', 'superglobals');
$t->assertKeyExists('LOG_LEVEL key exists in $_SERVER', $_SERVER, 'LOG_LEVEL');
$t->done();
