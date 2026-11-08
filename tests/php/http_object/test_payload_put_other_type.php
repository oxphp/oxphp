<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A body whose Content-Type is neither JSON nor a form is not parsed, however
// much its bytes look like a form.
$t = new TestCase('payload_put_other_type', 'http_object');

$req = oxphp_http_request();

$t->assertSame('a body of another type is not parsed', $req->payload(), null);
$t->assertSame('so the default stands in', $req->payload(null, 'dflt'), 'dflt');
$t->assertSame('and the raw body is untouched', $req->body(), 'a=1&b=two');

$t->done();
