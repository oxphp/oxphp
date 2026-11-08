<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// PHP builds $_POST for the POST method only, so on any other method payload()
// parses the urlencoded body itself, and must leave the body readable for
// whatever else this request wants to do with it.
$t = new TestCase('payload_put_form', 'http_object');

$expected = ['a' => '1', 'b' => 'two', 7 => 'seven'];
$raw = 'a=1&b=two&7=seven';
$req = oxphp_http_request();

$t->assertNotEqual('the request is not a POST', $req->method(), 'POST');
$t->assertSame('payload() holds every field', $req->payload(), $expected);
$t->assertSame('payload("b")', $req->payload('b'), 'two');
$t->assertSame('payload("7")', $req->payload('7'), 'seven');
$t->assertSame('a second object parses the same body', oxphp_http_request()->payload('a'), '1');
$t->assertSame('$_POST stays as PHP left it', $_POST, []);
$t->assertSame('body() is the raw body', $req->body(), $raw);

[$post] = request_parse_body();
$t->assertSame('request_parse_body() still finds the body', $post, $expected);
$t->assertSame('php://input still reads the body', file_get_contents('php://input'), $raw);

$t->done();
