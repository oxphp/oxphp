<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('payload_json_invalid', 'http_object');
$req = oxphp_http_request();
$t->assertNull('payload() is null on unparseable JSON', $req->payload());
$t->assertSame('a default applies', $req->payload(null, 'dflt'), 'dflt');
$t->assertSame('a key lookup falls back too', $req->payload('a', 'dflt'), 'dflt');
$t->done();
