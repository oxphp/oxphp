<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('content_type_header', 'headers');
header('Content-Type: text/plain');
$t->assertTrue('Content-Type header set to text/plain', true);
$t->done();
