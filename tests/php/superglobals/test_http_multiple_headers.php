<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('http_multiple_headers', 'superglobals');
$t->assertKeyExists('HTTP_X_FIRST key exists', $_SERVER, 'HTTP_X_FIRST');
$t->assertEqual('HTTP_X_FIRST is aaa', $_SERVER['HTTP_X_FIRST'], 'aaa');
$t->assertKeyExists('HTTP_X_SECOND key exists', $_SERVER, 'HTTP_X_SECOND');
$t->assertEqual('HTTP_X_SECOND is bbb', $_SERVER['HTTP_X_SECOND'], 'bbb');
$t->done();
