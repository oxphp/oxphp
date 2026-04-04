<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('http_headers', 'superglobals');
$t->assertKeyExists('HTTP_X_CUSTOM_TEST key exists', $_SERVER, 'HTTP_X_CUSTOM_TEST');
$t->assertEqual('HTTP_X_CUSTOM_TEST is hello_world', $_SERVER['HTTP_X_CUSTOM_TEST'], 'hello_world');
$t->done();
