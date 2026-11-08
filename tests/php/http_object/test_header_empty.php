<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('header_empty', 'http_object');

$req = oxphp_http_request();

// A field with a zero-length value is legal HTTP (RFC 9110 §5.5), so "the client
// sent it empty" and "the client did not send it" have to stay two answers.

$t->assertSame('$_SERVER[HTTP_X_FOO] is ""', $_SERVER['HTTP_X_FOO'] ?? null, '');
$t->assertSame('headers()[x-foo] is ""', $req->headers()['x-foo'] ?? null, '');
$t->assertTrue('hasHeader(X-Foo) is true', $req->hasHeader('X-Foo'));
$t->assertSame('header(X-Foo) is "" not null', $req->header('X-Foo'), '');
$t->assertSame('header(X-Foo, default) ignores the default', $req->header('X-Foo', 'fallback'), '');

$t->assertFalse('hasHeader(X-Absent) is false', $req->hasHeader('X-Absent'));
$t->assertNull('header(X-Absent) is null', $req->header('X-Absent'));
$t->assertSame('header(X-Absent, default) is the default', $req->header('X-Absent', 'fallback'), 'fallback');

$t->done();
