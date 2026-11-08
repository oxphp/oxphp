<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// Outside POST a multipart body is left to request_parse_body(). payload() and
// file() have to agree that it is unparsed, and must not use it up.
$t = new TestCase('payload_put_multipart', 'http_object');

$req = oxphp_http_request();

$t->assertNotEqual('the request is not a POST', $req->method(), 'POST');
$t->assertSame('payload() is empty', $req->payload(), []);
$t->assertNull('file("doc") is null', $req->file('doc'));
$t->assertSame('files() is empty', $req->files(), []);
$t->assertContains('body() is the raw body', $req->body(), 'fieldval');

[$post, $files] = request_parse_body();
$t->assertSame('request_parse_body() finds the field', $post, ['field' => 'fieldval']);
$t->assertSame('request_parse_body() finds the file', $files['doc']['name'] ?? null, 'small.txt');

$t->done();
