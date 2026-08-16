<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('query_parsed', 'http_object');
$req = oxphp_http_request();
$t->assertSame('query("key") === "val"', $req->query('key'), 'val');
$t->assertSame('query("num") === "42"', $req->query('num'), '42');
$all = $req->query();
$t->assertTrue('query() returns array', is_array($all));
$t->assertKeyExists('query() array has "key"', $all, 'key');
$t->assertKeyExists('query() array has "num"', $all, 'num');
$t->done();
