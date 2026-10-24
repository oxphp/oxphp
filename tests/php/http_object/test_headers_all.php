<?php
require_once __DIR__ . '/../test_helper.php';
$t = new TestCase('headers_all', 'http_object');

// The runner sends `X-Case-Probe: One`, over HTTP/1.1 and HTTP/2. headers()
// lists every name in lowercase, whatever case the client used; the value
// keeps its own.

$req = oxphp_http_request();
$headers = $req->headers();
$t->assertTrue('headers() returns array', is_array($headers));
$t->assertNotEmpty('headers() is not empty', $headers);
$t->assertSame('headers()[x-case-probe] is the value as sent', $headers['x-case-probe'] ?? null, 'One');
$t->assertKeyMissing('headers() has no X-Case-Probe key', $headers, 'X-Case-Probe');
$keys = array_map('strval', array_keys($headers));
$t->assertSame('every key is lowercase', $keys, array_map('strtolower', $keys));
$t->done();
