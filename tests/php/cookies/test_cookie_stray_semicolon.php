<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('cookie_stray_semicolon', 'cookies');

// The runner sends plugin cookies with a stray ';' before, after or between
// them, and with ?app=1 the application cookie a=1 as well. Stripping the
// plugin cookies leaves no empty pair behind: with nothing else sent there is
// no Cookie header at all, and next to a=1 the header is exactly a=1.

$expected = isset($_GET['app']) ? 'a=1' : null;

$t->assertSame('$_COOKIE', $_COOKIE, $expected === null ? [] : ['a' => '1']);
$t->assertSame('$_SERVER[HTTP_COOKIE]', $_SERVER['HTTP_COOKIE'] ?? null, $expected);

$req = oxphp_http_request();
$t->assertSame('header(cookie) agrees with $_SERVER', $req->header('cookie'), $expected);
$t->assertSame('headers()[cookie] agrees with $_SERVER', $req->headers()['cookie'] ?? null, $expected);
$t->assertSame('hasHeader(cookie) agrees with $_SERVER', $req->hasHeader('cookie'), $expected !== null);

$t->done();
