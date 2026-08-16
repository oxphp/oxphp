<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('path_info_split', 'pathinfo');
// Runner hits: /tests/pathinfo/test_path_info_split.php/extra/path
$t->assertKeyExists('PATH_INFO key exists', $_SERVER, 'PATH_INFO');
$t->assertSame('PATH_INFO equals /extra/path', $_SERVER['PATH_INFO'], '/extra/path');
$t->assertKeyExists('SCRIPT_NAME key exists', $_SERVER, 'SCRIPT_NAME');
$t->assertContains('SCRIPT_NAME contains filename', $_SERVER['SCRIPT_NAME'], 'test_path_info_split.php');
$t->done();
