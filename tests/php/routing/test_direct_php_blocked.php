<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('direct_php_blocked', 'routing');
// In framework mode direct PHP file access is blocked (404). The runner
// hits this file directly and expects a 404 response. This PHP file
// should never execute in framework mode; if it does, the block is broken.
$t->assertTrue('file reached (runner validates direct PHP access returns 404)', true);
$t->done();
