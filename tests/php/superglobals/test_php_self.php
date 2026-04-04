<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('php_self', 'superglobals');
$t->assertEqual('PHP_SELF equals SCRIPT_NAME', $_SERVER['PHP_SELF'], $_SERVER['SCRIPT_NAME']);
$t->done();
