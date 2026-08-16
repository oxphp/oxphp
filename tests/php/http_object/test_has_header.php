<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('has_header', 'http_object');
$req = oxphp_http_request();
$t->assertTrue('hasHeader("Host") is true', $req->hasHeader('Host'));
$t->assertFalse('hasHeader("X-Nope") is false', $req->hasHeader('X-Nope'));
$t->done();
