<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('body', 'http_object');
$req = oxphp_http_request();
$t->assertSame('body() === "request body content"', $req->body(), 'request body content');
$t->done();
