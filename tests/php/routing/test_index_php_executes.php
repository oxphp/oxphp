<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('index_php_executes', 'routing');
// Runner-side test: in framework mode a direct request to /index.php
// is now rewritten onto the front controller (no longer 404). The
// runner validates the HTTP 200 response; this PHP file is a placeholder
// that should never run on its own.
$t->assertTrue('file reached (runner validates /index.php direct access executes)', true);
$t->done();
