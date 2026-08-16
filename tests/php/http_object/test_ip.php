<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('ip', 'http_object');
$req = oxphp_http_request();
$t->assertNotEmpty('ip() is not empty', $req->ip());
$t->assertMatch('ip() matches IP pattern', $req->ip(), '/^[\d.:a-fA-F]+$/');
$t->done();
