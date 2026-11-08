<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// On POST payload() returns what PHP's form reader built, and that reader works
// by length. On any other method payload() uses the parser parse_str() uses, which
// works on a C string, so the same bytes give different fields.
$t = new TestCase('payload_form_raw_nul', 'http_object');

$req = oxphp_http_request();

if ($req->method() === 'POST') {
    $t->assertSame('POST keeps the byte', $req->payload('x'), "a\0b");
    $t->assertSame('and the field after it', $req->payload('y'), '2');
} else {
    parse_str($req->body(), $parsed);
    $t->assertSame('the result is what parse_str() gives', $req->payload(), $parsed);
    $t->assertSame('which stops at the byte', $req->payload(), ['x' => 'a']);
}

$t->done();
