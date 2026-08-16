<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('percent_decoding', 'routing');
$t->assertKeyExists('REQUEST_URI key exists', $_SERVER, 'REQUEST_URI');
// The runner hits this file via a percent-encoded URL. The fact this file
// was reached proves the server decoded the path correctly.
$t->assertContains('REQUEST_URI contains filename', $_SERVER['REQUEST_URI'], 'test_percent_decoding.php');
$t->done();
