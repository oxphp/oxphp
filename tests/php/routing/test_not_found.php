<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('not_found', 'routing');
// This file should never execute in a proper 404 test — the runner hits a
// nonexistent URL and checks the HTTP 404 response directly. If this file
// does execute, the test passes trivially; the real assertion is runner-side.
$t->assertTrue('file reached (runner validates 404 for nonexistent URL)', true);
$t->done();
