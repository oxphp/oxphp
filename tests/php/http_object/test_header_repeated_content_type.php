<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('header_repeated_content_type', 'http_object');

// The runner posts a body with Content-Type sent as two field lines.
// Content-Type allows one value only, so the CGI variable and the header
// accessors all read the first line, the one PHP parses the body by.

$req = oxphp_http_request();
$first = 'text/plain';

$t->assertSame('$_SERVER[CONTENT_TYPE] is the first line', $_SERVER['CONTENT_TYPE'] ?? null, $first);
$t->assertSame('contentType() agrees with $_SERVER', $req->contentType(), $first);
$t->assertSame('header(content-type) agrees with $_SERVER', $req->header('content-type'), $first);
$t->assertSame('headers()[content-type] agrees with $_SERVER', $req->headers()['content-type'] ?? null, $first);
$t->assertSame('the body arrives', file_get_contents('php://input'), 'body');

$t->done();
