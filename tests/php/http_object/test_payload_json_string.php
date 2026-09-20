<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('payload_json_string', 'http_object');
$req = oxphp_http_request();
$t->assertSame('payload() === "hello"', $req->payload(), 'hello');
$t->assertSame('a key lookup on a scalar falls back', $req->payload('hello', 'dflt'), 'dflt');
$t->done();
