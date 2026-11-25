<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('query_after_array_walk', 'http_object');
// array_walk() walks an object's declared properties whatever their
// visibility and leaves every slot it visits a reference, the one query()
// caches its array in included. PHP 8.6 deprecates passing an object to
// either walk but still walks it.
$req = oxphp_http_request();
ignoring_deprecations(static fn () => array_walk($req, static function (&$v) {}));
$t->assertSame("query('a') parses through the reference", $req->query('a'), ['1', '2']);
$t->assertSame('query() matches $_GET', $req->query(), $_GET);
$t->assertSame("query('a') reads the cached array through it", $req->query('a'), ['1', '2']);

// array_walk_recursive() goes on into the cached array and leaves its
// elements references too. A foreach by reference over what query('a')
// returned writes into the cache if that is the element's reference.
ignoring_deprecations(static fn () => array_walk_recursive($req, static function ($v) {}));
foreach ($req->query('a') as &$v) {
    $v = 'changed';
}
unset($v);
$t->assertSame("query('a') hands out a copy of the cached element", $req->query('a'), ['1', '2']);
$t->done();
