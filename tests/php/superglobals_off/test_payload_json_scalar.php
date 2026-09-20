<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A top-level JSON scalar, in the profile that also runs in worker mode —
// the parsed value is cached on the Request object, and a worker serves many
// requests through the same class.
$t = new TestCase('payload_json_scalar', 'superglobals_off');

$req = oxphp_http_request();

$t->assertSame('payload() === "hello"', $req->payload(), 'hello');
$t->assertSame('a second call reads the same cache', $req->payload(), 'hello');
$t->assertSame('a fresh request object agrees', oxphp_http_request()->payload(), 'hello');

$t->done();
