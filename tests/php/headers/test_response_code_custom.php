<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('response_code_custom', 'headers');
http_response_code(201);
$t->assertSame('http_response_code() returns 201', http_response_code(), 201);
$t->done();
