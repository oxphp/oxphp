<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('script_filename', 'superglobals');
$t->assertContains('SCRIPT_FILENAME contains current filename', $_SERVER['SCRIPT_FILENAME'], 'test_script_filename.php');
$t->assertTrue('SCRIPT_FILENAME file exists', file_exists($_SERVER['SCRIPT_FILENAME']));
$t->done();
