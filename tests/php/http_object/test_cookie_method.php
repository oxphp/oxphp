<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('cookie_method', 'http_object');
$req = oxphp_http_request();
$t->assertSame('cookie("name") === "testvalue"', $req->cookie('name'), 'testvalue');
$t->done();
