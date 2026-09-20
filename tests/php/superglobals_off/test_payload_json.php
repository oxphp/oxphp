<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// The JSON branch never read the flag, and must keep not reading it.
$t = new TestCase('payload_json', 'superglobals_off');

$req = oxphp_http_request();

$t->assertSame('payload("key")', $req->payload('key'), 'value');
$t->assertSame('payload("num")', $req->payload('num'), 42);
$t->assertEqual('payload() is the decoded object', $req->payload(), ['key' => 'value', 'num' => 42]);

$t->done();
