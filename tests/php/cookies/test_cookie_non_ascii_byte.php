<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('cookie_non_ascii_byte', 'cookies');

// The runner sends the byte 0xFF in a Cookie header, next to an application
// cookie and a plugin cookie, on one field line or split across two, over
// HTTP/1.1 and HTTP/2. Such a header is dropped as a whole, so no reader gets
// any of it.

$t->assertSame('$_COOKIE is empty', $_COOKIE, []);
$t->assertSame('$_SERVER has no HTTP_COOKIE', $_SERVER['HTTP_COOKIE'] ?? null, null);

$req = oxphp_http_request();
$t->assertSame('cookies() is empty', $req->cookies(), []);
$t->assertSame('header(cookie) is absent', $req->header('cookie'), null);
$t->assertSame('headers() has no cookie', $req->headers()['cookie'] ?? null, null);

$t->done();
