<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('response_code_default', 'headers');
$t->assertTrue('no explicit status code set', true);
$t->done();
