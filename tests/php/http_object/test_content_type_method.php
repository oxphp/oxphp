<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('content_type_method', 'http_object');
$req = oxphp_http_request();
$t->assertSame('contentType() === "application/json"', $req->contentType(), 'application/json');
$t->done();
