<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('request_object_exists', 'http_object');
$req = oxphp_http_request();
$t->assertNotNull('oxphp_http_request() returns non-null', $req);
$t->assertTrue('oxphp_http_request() returns an object', is_object($req));
$t->done();
