<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A request that suspends must come back to its own request object.
//
// `OxPHP\Http\Request` reads the per-request state the server holds on the
// worker thread, one slot per thread rather than one per request. Every new
// request overwrites it, so a fiber parked in a hooked sleep used to resume
// reading the path, headers, cookies, query and body of whichever request the
// worker served in the meantime — one client's request handed to another. The
// superglobals of a parked request are already carried across the suspension,
// which makes the split visible from inside a single request: `$_GET` and
// `$request->query()` disagree from the suspension point on.
//
// PHP_WORKERS=1, so the inner self-request below can only be served while this
// fiber is suspended, which is exactly the window the defect needs. It carries a
// header, a cookie, a query string and a body this request does not have, so a
// bleed shows up as the intruder's values rather than as emptiness.

$t = new TestCase('request_object_survives_suspend', 'hooks');

$request = oxphp_http_request();

$t->assertSame('outer request has its own probe in the query', $request->query('probe'), 'outer');
$t->assertKeyMissing('outer request carries no marker header', $request->headers(), 'x-marker');

$before = [
    'path'    => $request->path(),
    'probe'   => $request->query('probe'),
    'headers' => $request->headers(),
    'cookies' => $request->cookies(),
    'body'    => $request->body(),
    'id'      => oxphp_request_id(),
];

$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue('inner self-request socket connected', $sock !== false);
stream_set_timeout($sock, 5);

$innerBody = 'intruder-body=1';
fwrite($sock, "POST /tests/hooks/fixture_inner_state.php?tag=intruder&probe=inner HTTP/1.0\r\n"
    . "Host: 127.0.0.1\r\n"
    . "X-Marker: intruder\r\n"
    . "Cookie: ox_intruder=1\r\n"
    . "Content-Type: application/x-www-form-urlencoded\r\n"
    . "Content-Length: " . strlen($innerBody) . "\r\n"
    . "Connection: close\r\n\r\n"
    . $innerBody);

sleep(2);                                   // hooked: suspends this request fiber

$resp = (string) stream_get_contents($sock);
fclose($sock);

// Without this the rest proves nothing: if the intruder never ran inside the
// suspended window, the request state was never at risk in the first place.
$t->assertContains('intruder was served while this request was parked', $resp, 'INNER-OK');
$t->assertContains('intruder ran with its own query string', $resp, '"tag":"intruder"');

$t->assertSame('path() survived the suspend', $request->path(), $before['path']);
$t->assertSame('query() survived the suspend', $request->query('probe'), $before['probe']);
$t->assertSame('headers() survived the suspend', $request->headers(), $before['headers']);
$t->assertSame('cookies() survived the suspend', $request->cookies(), $before['cookies']);
$t->assertSame('body() survived the suspend', $request->body(), $before['body']);
$t->assertSame('the request id survived the suspend', oxphp_request_id(), $before['id']);

// These are the values the intruder would have left behind, so a failure here
// says what leaked.
$t->assertKeyMissing('headers() did not pick up the intruder\'s marker',
    $request->headers(), 'x-marker');
$t->assertKeyMissing('cookies() did not pick up the intruder\'s cookie',
    $request->cookies(), 'ox_intruder');
$t->assertNotContains('body() did not pick up the intruder\'s body',
    $request->body(), 'intruder-body');

// The point of the whole thing: the two views of one request must agree. The
// superglobals are carried across a suspension already, so a request object
// that is not says two different things about the request serving it.
$t->assertSame('query() and $_GET agree after the suspend',
    $request->query('probe'), $_GET['probe'] ?? null);
$t->assertSame('path() and $_SERVER[REQUEST_URI] agree after the suspend',
    $request->path(), parse_url($_SERVER['REQUEST_URI'] ?? '', PHP_URL_PATH));

$t->done();
