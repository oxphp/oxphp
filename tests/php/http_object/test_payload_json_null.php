<?php
require_once __DIR__ . '/../test_helper.php';
// A literal `null` body decodes to null, which is also what "no body to
// return" looks like — so the default applies, exactly as for invalid JSON.
$t = new TestCase('payload_json_null', 'http_object');
$req = oxphp_http_request();
$t->assertNull('payload() is null', $req->payload());
$t->assertSame('a default applies', $req->payload(null, 'dflt'), 'dflt');
$t->done();
