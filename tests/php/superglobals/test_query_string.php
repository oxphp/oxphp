<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('query_string', 'superglobals');
$t->assertKeyExists('QUERY_STRING key exists', $_SERVER, 'QUERY_STRING');
$t->assertContains('QUERY_STRING contains foo=bar', $_SERVER['QUERY_STRING'], 'foo=bar');
$t->done();
