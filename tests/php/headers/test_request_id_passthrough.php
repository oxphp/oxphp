<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('request_id_passthrough', 'headers');
$t->assertSame('HTTP_X_REQUEST_ID is abc123', $_SERVER['HTTP_X_REQUEST_ID'] ?? null, 'abc123');
$t->done();
