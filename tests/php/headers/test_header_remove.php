<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('header_remove', 'headers');
header('X-Del: val');
header_remove('X-Del');
$headersList = implode("\n", headers_list());
$t->assertNotContains('headers_list does not contain X-Del', $headersList, 'X-Del');
$t->done();
