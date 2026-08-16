<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('is_secure', 'http_object');
$req = oxphp_http_request();
$t->assertFalse('isSecure() is false (non-TLS)', $req->isSecure());
$t->done();
