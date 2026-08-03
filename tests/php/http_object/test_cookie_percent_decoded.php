<?php
require __DIR__ . '/../test_helper.php';
$t = new TestCase('cookie_percent_decoded', 'http_object');

$req = oxphp_http_request();

// Cookies decode differently from query strings: PHP runs values through
// php_raw_url_decode (percent-escapes only, "+" stays literal) and leaves
// names untouched. The object API must follow the same rules, not the
// query-string ones.

// === Reference behaviour: $_COOKIE ===
$t->assertSame('$_COOKIE[name] is decoded UTF-8', $_COOKIE['name'] ?? null, 'Привет');
$t->assertSame('$_COOKIE[plus] keeps "+" literal', $_COOKIE['plus'] ?? null, 'a+b');
$t->assertSame('$_COOKIE[space] decodes %20', $_COOKIE['space'] ?? null, 'a b');
$t->assertKeyExists(
    '$_COOKIE keeps the name percent-encoded',
    $_COOKIE,
    '%D0%BA%D0%BB%D1%8E%D1%87'
);

// === cookie($name) must return what $_COOKIE returns ===
$t->assertSame('cookie(name) is decoded UTF-8', $req->cookie('name'), 'Привет');
$t->assertSame('cookie(plus) keeps "+" literal', $req->cookie('plus'), 'a+b');
$t->assertSame('cookie(space) decodes %20', $req->cookie('space'), 'a b');
$t->assertSame(
    'cookie() is looked up by the undecoded name',
    $req->cookie('%D0%BA%D0%BB%D1%8E%D1%87'),
    'знач'
);

// === cookies() array form must agree too ===
$all = $req->cookies();
$t->assertTrue('cookies() returns an array', is_array($all));
$t->assertSame('cookies()[name] is decoded UTF-8', $all['name'] ?? null, 'Привет');
$t->assertSame('cookies()[plus] keeps "+" literal', $all['plus'] ?? null, 'a+b');
$t->assertKeyExists(
    'cookies() keeps the name percent-encoded',
    $all,
    '%D0%BA%D0%BB%D1%8E%D1%87'
);

// === Parity ===
foreach (['name', 'plus', 'space', '%D0%BA%D0%BB%D1%8E%D1%87'] as $k) {
    $t->assertSame("cookie('$k') === \$_COOKIE['$k']", $req->cookie($k), $_COOKIE[$k] ?? null);
    $t->assertSame("cookies()['$k'] === \$_COOKIE['$k']", $all[$k] ?? null, $_COOKIE[$k] ?? null);
}

$t->done();
