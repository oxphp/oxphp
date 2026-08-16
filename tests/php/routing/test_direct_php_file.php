<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('direct_php_file', 'routing');
$t->assertTrue('file executed via direct PHP access', true);
$t->done();
