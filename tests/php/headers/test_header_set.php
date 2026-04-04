<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('header_set', 'headers');
header('X-Custom: test_value');
$t->assertTrue('header() called without error', true);
$t->done();
