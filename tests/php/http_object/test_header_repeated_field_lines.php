<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('header_repeated_field_lines', 'http_object');

// The runner sends X-Foo and X-Forwarded-For as two field lines each, over
// HTTP/1.1 and HTTP/2. Every way PHP reads a header gives the same value: the
// lines joined with ", " in the order they arrived.

$req = oxphp_http_request();

$t->assertSame('$_SERVER[HTTP_X_FOO] is both lines joined', $_SERVER['HTTP_X_FOO'] ?? null, 'one, two');
$t->assertSame('header(x-foo) agrees with $_SERVER', $req->header('x-foo'), 'one, two');
$t->assertSame('headers()[x-foo] agrees with $_SERVER', $req->headers()['x-foo'] ?? null, 'one, two');

$xff = '198.51.100.1, 203.0.113.9';
$t->assertSame('$_SERVER[HTTP_X_FORWARDED_FOR] is both lines joined', $_SERVER['HTTP_X_FORWARDED_FOR'] ?? null, $xff);
$t->assertSame('header(X-Forwarded-For) agrees with $_SERVER', $req->header('X-Forwarded-For'), $xff);
$t->assertSame('headers()[x-forwarded-for] agrees with $_SERVER', $req->headers()['x-forwarded-for'] ?? null, $xff);

$t->done();
