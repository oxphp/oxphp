<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('host_from_authority', 'http_object');

// The runner asks for h2.example:8443 over HTTP/1.1 and over HTTP/2. Over
// HTTP/2 curl sends the host as the :authority pseudo-header and no Host
// field, so everything below has to come from the :authority. Every way PHP
// reads the host gives the same answer on both.

$req = oxphp_http_request();
$case = $_GET['case'] ?? '';

$t->assertSame('the request came over the protocol its case names', $_SERVER['SERVER_PROTOCOL'] ?? null, $case === 'h2' ? 'HTTP/2' : 'HTTP/1.1');

$t->assertSame('$_SERVER[HTTP_HOST]', $_SERVER['HTTP_HOST'] ?? null, 'h2.example:8443');
$t->assertSame('$_SERVER[SERVER_NAME]', $_SERVER['SERVER_NAME'] ?? null, 'h2.example');
$t->assertSame('$_SERVER[SERVER_PORT]', $_SERVER['SERVER_PORT'] ?? null, '8443');
$t->assertSame('host()', $req->host(), 'h2.example');
$t->assertSame('port()', $req->port(), 8443);
$t->assertSame('header(host)', $req->header('host'), 'h2.example:8443');
$t->assertSame('headers()[host]', $req->headers()['host'] ?? null, 'h2.example:8443');
$t->assertSame('fullUri()', $req->fullUri(), 'http://h2.example:8443' . ($_SERVER['REQUEST_URI'] ?? ''));

$t->done();
