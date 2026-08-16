<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('cookies_all', 'http_object');
$req = oxphp_http_request();
$cookies = $req->cookies();
$t->assertTrue('cookies() returns array', is_array($cookies));
$t->assertKeyExists('cookies() has "a"', $cookies, 'a');
$t->assertKeyExists('cookies() has "b"', $cookies, 'b');
$t->assertSame('cookies()["a"] === "1"', $cookies['a'], '1');
$t->assertSame('cookies()["b"] === "2"', $cookies['b'], '2');
$t->done();
