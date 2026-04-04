<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('headers_all', 'http_object');
$req = oxphp_http_request();
$headers = $req->headers();
$t->assertTrue('headers() returns array', is_array($headers));
$t->assertNotEmpty('headers() is not empty', $headers);
$t->done();
