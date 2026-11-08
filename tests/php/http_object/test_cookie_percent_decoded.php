<?php
require_once __DIR__ . '/../test_helper.php';
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

// === A pair with no name is not a cookie ===
$t->assertKeyMissing('$_COOKIE drops a pair with no name', $_COOKIE, '');
$t->assertKeyMissing('cookies() drops a pair with no name', $all, '');
$t->assertNull("cookie('') finds nothing", $req->cookie(''));

// === A name with no "=" is a cookie with an empty value ===
$t->assertSame('$_COOKIE[flag] is ""', $_COOKIE['flag'] ?? null, '');
$t->assertSame('cookie(flag) is "" not null', $req->cookie('flag'), '');
$t->assertSame('cookies()[flag] is ""', $all['flag'] ?? null, '');

// === Only the name loses its leading spaces; the value is kept whole ===
$t->assertSame('$_COOKIE[trail] keeps the trailing space', $_COOKIE['trail'] ?? null, '1 ');
$t->assertSame('cookie(trail) keeps the trailing space', $req->cookie('trail'), '1 ');
$t->assertSame('cookies()[trail] keeps the trailing space', $all['trail'] ?? null, '1 ');

// === Of two cookies with one name, the first is the more specific path ===
$t->assertSame('$_COOKIE[sid] is the first of two', $_COOKIE['sid'] ?? null, 'app');
$t->assertSame('cookie(sid) is the first of two', $req->cookie('sid'), 'app');
$t->assertSame('cookies()[sid] is the first of two', $all['sid'] ?? null, 'app');

// === Parity ===
foreach (['name', 'plus', 'space', '%D0%BA%D0%BB%D1%8E%D1%87', 'flag', 'trail', 'sid'] as $k) {
    $t->assertSame("cookie('$k') === \$_COOKIE['$k']", $req->cookie($k), $_COOKIE[$k] ?? null);
    $t->assertSame("cookies()['$k'] === \$_COOKIE['$k']", $all[$k] ?? null, $_COOKIE[$k] ?? null);
}

$t->done();
