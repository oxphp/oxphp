<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('pathinfo_encoded', 'pathinfo');
// Runner hits: /tests/pathinfo/test_pathinfo_encoded.php/u%20ser
// PATH_INFO must be percent-decoded; SCRIPT_NAME must be the clean script path
// (no %20 leaking from the raw URI).
$t->assertKeyExists('PATH_INFO key exists', $_SERVER, 'PATH_INFO');
$t->assertSame('PATH_INFO is decoded', $_SERVER['PATH_INFO'], '/u ser');
$t->assertContains('SCRIPT_NAME has script', $_SERVER['SCRIPT_NAME'], 'test_pathinfo_encoded.php');
$t->assertNotContains('SCRIPT_NAME has no %20', $_SERVER['SCRIPT_NAME'], '%20');
$t->done();
