<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('index_php_blocked', 'routing');
// Runner-side test: in framework mode a direct request to /index.php
// should be blocked (404) to prevent double-execution. The runner
// validates the HTTP response; this PHP file is a placeholder.
$t->assertTrue('file reached (runner validates /index.php direct access blocked)', true);
$t->done();
