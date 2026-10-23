<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('header_repeated_non_ascii_line', 'http_object');

// The runner sends X-Bin as two field lines, the second carrying the byte
// 0xFF, and X-Ok as a control. X-Bin is left out as a whole, as it is when it
// arrives as one such line: PHP is not handed the readable lines as if they
// were all of it.

$req = oxphp_http_request();

$t->assertSame('the control header arrives', $_SERVER['HTTP_X_OK'] ?? null, 'yes');
$t->assertFalse('$_SERVER has no HTTP_X_BIN', array_key_exists('HTTP_X_BIN', $_SERVER));
$t->assertNull('header(x-bin) is null', $req->header('x-bin'));
$t->assertFalse('hasHeader(x-bin) is false', $req->hasHeader('x-bin'));
$t->assertFalse('headers() has no x-bin', array_key_exists('x-bin', $req->headers()));

$t->done();
