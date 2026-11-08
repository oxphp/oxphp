<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// This profile sets arg_separator.input to ";". payload() on a method other than
// POST uses the parser parse_str() uses, which honours the setting. PHP's form
// reader on POST splits on & whatever the setting is.
$t = new TestCase('payload_arg_separator', 'http_object');

$req = oxphp_http_request();
$t->assertSame('the profile changed the setting', ini_get('arg_separator.input'), ';');

if ($req->method() === 'POST') {
    $t->assertSame('POST splits on &', $req->payload(), ['a' => '1;b=2', 'c' => '3']);
} else {
    parse_str($req->body(), $parsed);
    $t->assertSame('the result is what parse_str() gives', $req->payload(), $parsed);
    $t->assertSame('which splits on ;', $req->payload(), ['a' => '1', 'b' => '2&c=3']);
}

$t->done();
