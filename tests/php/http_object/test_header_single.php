<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('header_single', 'http_object');
$req = oxphp_http_request();
$t->assertNotNull('header("Host") is not null', $req->header('Host'));
$t->done();
