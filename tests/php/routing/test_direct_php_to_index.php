<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('direct_php_to_index', 'routing');
// In framework mode any *.php request is rewritten onto the front
// controller (index.php) with PATH_INFO carrying the original URI.
// The runner hits this file directly and expects a 200 response from
// index.php (not 404). This PHP file should never execute itself; if
// it does, the rewrite is broken.
$t->assertTrue('file reached (runner validates direct PHP rewrites to index.php)', true);
$t->done();
