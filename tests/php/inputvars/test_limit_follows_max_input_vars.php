<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('limit_follows_max_input_vars', 'inputvars');

$req = oxphp_http_request();
$limit = (int) ini_get('max_input_vars');

// === The premise: a limit above PHP's default, and a request past it ===
$t->assertSame('the profile raises max_input_vars above 1000', $limit, 1200);
$t->assertSame('the query string carries 1300 parameters', substr_count((string) $req->queryString(), '='), 1300);
$t->assertSame('the Cookie header carries 1300 cookies', substr_count((string) $req->header('Cookie'), '='), 1300);

// === Query: both APIs stop at the same parameter ===
$t->assertSame('$_GET holds exactly the limit', count($_GET), $limit);
$t->assertSame('query() holds exactly the limit', count($req->query()), $limit);
$t->assertSame('query() reads the last parameter inside the limit', $req->query("k$limit"), '1');
$t->assertNull('query() does not read past the limit', $req->query('k' . ($limit + 1)));

// === Cookies: PHP applies the same limit to $_COOKIE ===
$t->assertSame('$_COOKIE holds exactly the limit', count($_COOKIE), $limit);
$t->assertSame('cookies() holds exactly the limit', count($req->cookies()), $limit);
$t->assertSame('cookie() reads the last cookie inside the limit', $req->cookie("c$limit"), '1');
$t->assertNull('cookie() does not read past the limit', $req->cookie('c' . ($limit + 1)));

$t->done();
