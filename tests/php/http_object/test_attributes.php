<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('attributes', 'http_object');
$req = oxphp_http_request();
$attrs = $req->attributes();
$attrs->set('k', 'v');
$t->assertSame('attributes()->get("k") === "v"', $attrs->get('k'), 'v');
$t->done();
