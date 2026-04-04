<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('http_protocol', 'http_object');
$req = oxphp_http_request();
$t->assertMatch('httpProtocol() matches /^HTTP\/[12]/', $req->httpProtocol(), '/^HTTP\/[12]/');
$t->done();
