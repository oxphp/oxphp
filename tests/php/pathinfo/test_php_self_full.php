<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('php_self_full', 'pathinfo');
// Runner hits: /tests/pathinfo/test_php_self_full.php/extra/path
// PHP_SELF should combine the script name with the path info.
$t->assertKeyExists('PHP_SELF key exists', $_SERVER, 'PHP_SELF');
$t->assertContains('PHP_SELF contains filename', $_SERVER['PHP_SELF'], 'test_php_self_full.php');
$t->assertContains('PHP_SELF contains path info', $_SERVER['PHP_SELF'], '/extra/path');
$t->done();
