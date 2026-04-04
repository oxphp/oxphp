<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('port', 'http_object');
$req = oxphp_http_request();
$t->assertGreaterThan('port() > 0', $req->port(), 0);
$t->done();
