<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('host', 'http_object');
$req = oxphp_http_request();
$t->assertNotEmpty('host() is not empty', $req->host());
$t->done();
