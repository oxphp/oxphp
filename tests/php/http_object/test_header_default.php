<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('header_default', 'http_object');
$req = oxphp_http_request();
$t->assertSame('header("X-Nonexistent", "default_val") === "default_val"', $req->header('X-Nonexistent', 'default_val'), 'default_val');
$t->done();
