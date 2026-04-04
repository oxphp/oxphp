<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('no_path_info', 'pathinfo');
// Runner hits this file normally, with no extra path segments.
// PATH_INFO must be absent from $_SERVER when there is no path info.
$t->assertKeyMissing('PATH_INFO absent from SERVER', $_SERVER, 'PATH_INFO');
$t->assertKeyExists('SCRIPT_NAME key exists', $_SERVER, 'SCRIPT_NAME');
$t->assertContains('SCRIPT_NAME contains filename', $_SERVER['SCRIPT_NAME'], 'test_no_path_info.php');
$t->done();
