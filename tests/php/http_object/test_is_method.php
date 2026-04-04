<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('is_method', 'http_object');
$req = oxphp_http_request();
$t->assertTrue('isMethod("get") is true (case-insensitive)', $req->isMethod('get'));
$t->assertFalse('isMethod("POST") is false', $req->isMethod('POST'));
$t->done();
