<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('payload_json_int', 'http_object');
$req = oxphp_http_request();
$t->assertSame('payload() === 42', $req->payload(), 42);
$t->assertType('payload() is an int', $req->payload(), 'integer');
$t->done();
