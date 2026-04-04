<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('http_protocol_version', 'http_object');
$req = oxphp_http_request();
$t->assertMatch('httpProtocolVersion() matches /^[12]/', $req->httpProtocolVersion(), '/^[12]/');
$t->done();
