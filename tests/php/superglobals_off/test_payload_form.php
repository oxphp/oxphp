<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// SUPERGLOBALS_ENABLED=false does not take $_POST away (see
// test_body_superglobals), so payload() has the parsed body sitting right
// beside it and must hand it over.
$t = new TestCase('payload_form', 'superglobals_off');

$req = oxphp_http_request();

$t->assertSame('payload("a") === "1"', $req->payload('a'), '1');
$t->assertSame('payload("b") === "two"', $req->payload('b'), 'two');
$t->assertEqual('payload() is the whole body', $req->payload(), ['a' => '1', 'b' => 'two']);

$t->done();
