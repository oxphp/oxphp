<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('scheme', 'http_object');
$req = oxphp_http_request();
$t->assertSame('scheme() is "http"', $req->scheme(), 'http');
$t->done();
