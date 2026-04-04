<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('nested_directory', 'routing');
$t->assertTrue('file in nested path executed', true);
$t->assertContains('SCRIPT_NAME contains filename', $_SERVER['SCRIPT_NAME'], 'test_nested_directory.php');
$t->done();
