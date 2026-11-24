<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('query_percent_decoded', 'http_object');

$req = oxphp_http_request();

// === Reference behaviour: $_GET decodes percent-escapes and "+" ===
$t->assertSame('$_GET[name] is decoded UTF-8', $_GET['name'] ?? null, 'Привет');
$t->assertSame('$_GET[plus] turns "+" into a space', $_GET['plus'] ?? null, 'a b');
$t->assertSame('$_GET[space] decodes %20', $_GET['space'] ?? null, 'a b');
$t->assertSame('$_GET[amp] decodes %26 inside a value', $_GET['amp'] ?? null, 'a&b');
$t->assertSame('$_GET is keyed by the decoded key', $_GET['ключ'] ?? null, 'знач');

// === The raw form stays available through queryString() ===
// Decoding query() must not change what queryString() reports.
$t->assertContains('queryString() keeps the raw encoding', $req->queryString(), '%D0%9F');

// === query($key) must return what $_GET returns ===
$t->assertSame('query(name) is decoded UTF-8', $req->query('name'), 'Привет');
$t->assertSame('query(plus) turns "+" into a space', $req->query('plus'), 'a b');
$t->assertSame('query(space) decodes %20', $req->query('space'), 'a b');
$t->assertSame('query(amp) decodes %26 inside a value', $req->query('amp'), 'a&b');
$t->assertSame('query(ключ) resolves the decoded key', $req->query('ключ'), 'знач');

// === query() array form must agree too ===
$all = $req->query();
$t->assertTrue('query() returns an array', is_array($all));
$t->assertSame('query()[name] is decoded UTF-8', $all['name'] ?? null, 'Привет');
$t->assertSame('query()[plus] turns "+" into a space', $all['plus'] ?? null, 'a b');
$t->assertSame('query()[amp] decodes %26 inside a value', $all['amp'] ?? null, 'a&b');
$t->assertKeyExists('query() array is keyed by the decoded key', $all, 'ключ');

// === Names are mangled the way $_GET mangles them ===
$t->assertKeyExists('$_GET mangles "a.b" into "a_b"', $_GET, 'a_b');
$t->assertKeyExists('query() array has "a_b" too', $all, 'a_b');
$t->assertSame("query('a_b') reads it", $req->query('a_b'), '1');
$t->assertNull("query('a.b') finds nothing", $req->query('a.b'));

// === The whole array is $_GET ===
$t->assertSame('query() === $_GET', $all, $_GET);
foreach (['name', 'plus', 'space', 'amp', 'ключ', 'dup', 'a_b'] as $k) {
    $t->assertSame("query('$k') === \$_GET['$k']", $req->query($k), $_GET[$k] ?? null);
}

// === A repeated name reads as its last occurrence, in both forms ===
$t->assertSame('$_GET[dup] is the last of two', $_GET['dup'] ?? null, '2');
$t->assertSame('query(dup) is the last of two', $req->query('dup'), '2');
$t->assertSame('query()[dup] is the last of two', $all['dup'] ?? null, '2');

// === A pair with no name is not registered ===
$t->assertKeyMissing('$_GET has no empty name', $_GET, '');
$t->assertKeyMissing('query() has no empty name', $all, '');
$t->assertNull("query('') finds nothing", $req->query(''));

// === Metadata ===
$t->meta('query_string', $req->queryString());
$t->meta('query_all', $all);
$t->meta('get_all', $_GET);

$t->done();
