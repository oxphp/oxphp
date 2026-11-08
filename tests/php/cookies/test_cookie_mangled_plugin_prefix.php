<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('cookie_mangled_plugin_prefix', 'cookies');

// The runner sends the application cookie a=1 next to one whose name PHP would
// register under the reserved __oxp_ prefix, since it turns a space, a dot and
// an unmatched bracket in a cookie name into an underscore.

$t->assertSame('$_COOKIE holds only the application cookie', $_COOKIE, ['a' => '1']);
$t->assertSame('$_SERVER[HTTP_COOKIE] holds only the application cookie', $_SERVER['HTTP_COOKIE'] ?? null, 'a=1');

$req = oxphp_http_request();
$t->assertSame('cookies() agrees with $_COOKIE', $req->cookies(), $_COOKIE);
$t->assertSame('header(cookie) agrees with $_SERVER', $req->header('cookie'), 'a=1');

$t->done();
