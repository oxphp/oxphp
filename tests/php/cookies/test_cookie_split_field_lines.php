<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('cookie_split_field_lines', 'cookies');

// The runner sends the cookies a=1, b=2 and the plugin cookie __oxp_t_s=1
// split across two Cookie field lines, in either order, over HTTP/1.1 and
// HTTP/2. However they are split, PHP sees one header: the lines joined with
// "; " and the plugin cookie stripped.

$t->assertSame('$_COOKIE holds both application cookies and nothing else', $_COOKIE, ['a' => '1', 'b' => '2']);
$t->assertSame('$_SERVER[HTTP_COOKIE] is the joined header', $_SERVER['HTTP_COOKIE'] ?? null, 'a=1; b=2');

$req = oxphp_http_request();
$t->assertSame('cookies() agrees with $_COOKIE', $req->cookies(), $_COOKIE);
$t->assertSame('header(cookie) agrees with $_SERVER', $req->header('cookie'), $_SERVER['HTTP_COOKIE'] ?? null);
$t->assertSame('headers()[cookie] agrees with $_SERVER', $req->headers()['cookie'] ?? null, $_SERVER['HTTP_COOKIE'] ?? null);

$t->done();
