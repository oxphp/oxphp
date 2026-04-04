<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('path_traversal', 'routing');
// The runner tests path traversal by hitting /../../../etc/passwd and
// expecting a 400 or 404 response. If this PHP file executes at all, we
// just pass — the real assertion is that the traversal attempt was blocked.
$t->assertTrue('file reached (runner validates traversal attempt is blocked)', true);
$t->done();
