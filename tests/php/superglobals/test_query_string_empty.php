<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('query_string_empty', 'superglobals');
$t->assertSame('QUERY_STRING is empty string', $_SERVER['QUERY_STRING'], '');
$t->done();
