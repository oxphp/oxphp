<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('script_name', 'superglobals');
$t->assertContains('SCRIPT_NAME contains filename', $_SERVER['SCRIPT_NAME'], 'test_script_name.php');
$t->done();
