<?php
require __DIR__ . '/../test_helper.php';
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

// === The names PHP mangles are a deliberate divergence, not an oversight ===
// php_register_variable_ex rewrites ' ', '.' and '[' in superglobal keys;
// query() reports the name the client sent. Pinned so the difference stays
// intentional and cannot drift back by accident.
$t->assertKeyExists('$_GET mangles "a.b" into "a_b"', $_GET, 'a_b');
$t->assertKeyMissing('$_GET has no literal "a.b"', $_GET, 'a.b');
$t->assertSame('query() keeps the name as sent', $req->query('a.b'), '1');
$t->assertKeyExists('query() array keeps "a.b"', $all, 'a.b');

// === Parity on names that survive PHP's key mangling ===
// Restricted on purpose: for a mangled name the two APIs are expected to
// differ, so a blanket comparison would assert the wrong thing.
foreach (['name', 'plus', 'space', 'amp', 'ключ'] as $k) {
    $t->assertSame("query('$k') === \$_GET['$k']", $req->query($k), $_GET[$k] ?? null);
    $t->assertSame("query()['$k'] === \$_GET['$k']", $all[$k] ?? null, $_GET[$k] ?? null);
}

// === Metadata ===
$t->meta('query_string', $req->queryString());
$t->meta('query_all', $all);
$t->meta('get_all', $_GET);

$t->done();
