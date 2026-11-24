<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('query_ignores_get_writes', 'http_object');
// query() answers with the parameters the request sent, not with what the
// script has made of $_GET since.
$_GET['a'] = 'script';
unset($_GET['k']);
$req = oxphp_http_request();
$t->assertSame('query() holds the parameters as the request sent them', $req->query(), ['a' => ['1', '2'], 'k' => '1']);
$t->assertSame('$_GET keeps what the script wrote', $_GET, ['a' => 'script']);
$t->done();
