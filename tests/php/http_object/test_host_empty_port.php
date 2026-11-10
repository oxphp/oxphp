<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('host_empty_port', 'http_object');

// The runner sends `Host: a.example:`. RFC 3986 section 3.2.3 reads an empty
// port as the scheme's default, and every way PHP reads the port gives it.

$req = oxphp_http_request();

$t->assertSame('$_SERVER[HTTP_HOST]', $_SERVER['HTTP_HOST'] ?? null, 'a.example:');
$t->assertSame('$_SERVER[SERVER_NAME]', $_SERVER['SERVER_NAME'] ?? null, 'a.example');
$t->assertSame('$_SERVER[SERVER_PORT]', $_SERVER['SERVER_PORT'] ?? null, '80');
$t->assertSame('host()', $req->host(), 'a.example');
$t->assertSame('port()', $req->port(), 80);
$t->assertSame('fullUri()', $req->fullUri(), 'http://a.example' . ($_SERVER['REQUEST_URI'] ?? ''));

$t->done();
