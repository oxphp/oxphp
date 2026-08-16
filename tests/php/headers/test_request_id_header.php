<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('request_id_header', 'headers');
$requestId = oxphp_request_id();
$t->assertMatch('oxphp_request_id() matches 20-char hex pattern', $requestId, '/^[0-9a-f]{20}$/');
$t->done();
