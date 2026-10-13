<?php
// Sent: `X_Forwarded_For: 1.2.3.4` and `X-Custom-Test: hello_world`.
// `X_Forwarded_For` would land on HTTP_X_FORWARDED_FOR — the key of the
// `X-Forwarded-For` a proxy in front may set — so it must not reach $_SERVER.
// The object API still returns it under the name it was sent with.
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('header_name_with_underscore_not_registered', 'superglobals');
$t->assertKeyMissing('X_Forwarded_For does not create HTTP_X_FORWARDED_FOR', $_SERVER, 'HTTP_X_FORWARDED_FOR');
$t->assertSame('a hyphenated header in the same request still arrives', $_SERVER['HTTP_X_CUSTOM_TEST'] ?? null, 'hello_world');
$t->assertSame('header("X_Forwarded_For") returns the value', oxphp_http_request()->header('X_Forwarded_For'), '1.2.3.4');
$t->done();
