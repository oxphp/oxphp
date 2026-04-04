<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('payload_form', 'http_object');
$req = oxphp_http_request();
$t->assertSame('payload("key") === "value"', $req->payload('key'), 'value');
$t->assertSame('payload("num") === "42"', $req->payload('num'), '42');
$t->done();
