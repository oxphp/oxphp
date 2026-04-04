<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('method', 'http_object');
$req = oxphp_http_request();
$t->assertSame('method() returns GET', $req->method(), 'GET');
$t->done();
