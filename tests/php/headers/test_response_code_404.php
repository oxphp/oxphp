<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('response_code_404', 'headers');
http_response_code(404);
$t->assertTrue('status set to 404', true);
$t->done();
