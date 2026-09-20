<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// The other half of the measurement: what the flag does take away, and that
// the object API answers for all of it.
$t = new TestCase('suppressed_superglobals', 'superglobals_off');

$t->assertFalse('oxphp_superglobals_enabled() is false', oxphp_superglobals_enabled());

$t->assertEmpty('$_GET is empty', $_GET);
$t->assertKeyMissing('$_SERVER has no REQUEST_METHOD', $_SERVER, 'REQUEST_METHOD');
$t->assertKeyMissing('$_SERVER has no REQUEST_URI', $_SERVER, 'REQUEST_URI');
$t->assertKeyMissing('$_SERVER has no HTTP_HOST', $_SERVER, 'HTTP_HOST');

// Not an empty array, though: PHP registers these two itself, outside the
// SAPI callback the flag gates.
$t->assertKeyExists('$_SERVER still has REQUEST_TIME', $_SERVER, 'REQUEST_TIME');

$req = oxphp_http_request();
$t->assertSame('query("q") answers instead of $_GET', $req->query('q'), 'val');
$t->assertSame('method() answers instead of $_SERVER', $req->method(), 'GET');

$t->done();
