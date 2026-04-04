<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('query_string', 'http_object');
$req = oxphp_http_request();
$t->assertContains('queryString() contains "key=val"', $req->queryString(), 'key=val');
$t->done();
