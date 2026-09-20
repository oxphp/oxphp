<?php
require_once __DIR__ . '/../test_helper.php';
// `false` is the body that collides with a cache that encodes "parsed, nothing
// to return" as the boolean false. A default given alongside it must not win:
// the body decoded to a value, and that value is false.
$t = new TestCase('payload_json_false', 'http_object');
$req = oxphp_http_request();
$t->assertSame('payload() === false', $req->payload(), false);
$t->assertSame('a default does not replace it', $req->payload(null, 'dflt'), false);
$t->done();
