<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('payload_json_true', 'http_object');
$req = oxphp_http_request();
$t->assertSame('payload() === true', $req->payload(), true);
$t->done();
