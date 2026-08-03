<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('binary_values', 'http_object');

$req = oxphp_http_request();

// PHP strings are byte strings. A percent-escape can encode any byte, and
// signed cookies / binary session ids routinely contain bytes that are not
// valid UTF-8. Those must survive the accessor verbatim — replacing them
// would break an HMAC over the value without any error surfacing.
//
// Every comparison below is on bin2hex() forms, never on the raw bytes.
// The harness reports a failed assertion by putting `expected` and `actual`
// through json_encode(), which returns false for a string that is not valid
// UTF-8 — the whole response body would then be empty and the runner would
// report "non-JSON response" instead of a diff, exactly when this test has
// something to say. There is no meta() here for the same reason it would not
// help: this test runs in the runner's pipe form, which emits a fixed empty
// meta object, so anything put there is discarded.

$expectedHex = 'ff00fe80';

$getHex    = bin2hex((string) ($_GET['bin'] ?? ''));
$queryHex  = bin2hex((string) $req->query('bin'));
$cookieHex = bin2hex((string) ($_COOKIE['bin'] ?? ''));
$reqCkHex  = bin2hex((string) $req->cookie('bin'));

// === Query ===
$t->assertSame('$_GET[bin] keeps raw bytes', $getHex, $expectedHex);
$t->assertSame('query(bin) keeps raw bytes', $queryHex, $expectedHex);
$t->assertSame('query(bin) === $_GET[bin]', $queryHex, $getHex);
$t->assertSame('query(bin) is 4 bytes long', strlen((string) $req->query('bin')), 4);
$t->assertFalse(
    'query(bin) is not valid UTF-8 — no replacement happened',
    mb_check_encoding((string) $req->query('bin'), 'UTF-8')
);

// === Cookie ===
$t->assertSame('$_COOKIE[bin] keeps raw bytes', $cookieHex, $expectedHex);
$t->assertSame('cookie(bin) keeps raw bytes', $reqCkHex, $expectedHex);
$t->assertSame('cookie(bin) === $_COOKIE[bin]', $reqCkHex, $cookieHex);
$t->assertFalse(
    'cookie(bin) is not valid UTF-8 — no replacement happened',
    mb_check_encoding((string) $req->cookie('bin'), 'UTF-8')
);

// === Present-but-empty is "", not absent ===
$t->assertSame('$_GET[empty] is ""', $_GET['empty'] ?? null, '');
$t->assertSame('query(empty) is ""', $req->query('empty'), '');
$t->assertSame('$_COOKIE[empty] is ""', $_COOKIE['empty'] ?? null, '');
$t->assertSame('cookie(empty) is "" not null', $req->cookie('empty'), '');
$t->assertNull('cookie(absent) is null', $req->cookie('no_such_cookie'));
$t->assertNull('query(absent) is null', $req->query('no_such_param'));

$t->done();
