<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A request that suspends must come back to its own php://input.
//
// The stream php://input reads from is thread-wide state — the engine keeps it
// in SG(request_info).request_body, next to the flag saying the body has already
// been read in full. Every new request resets both, so a fiber parked in a
// hooked sleep used to resume reading the body of whichever request the worker
// served in the meantime, verbatim: with that flag set php://input never asks
// the SAPI for this request's bytes at all, it just rewinds the stream it finds
// and reads it.
//
// PHP_WORKERS=1, so the inner self-request below can only be served while this
// fiber is suspended. It reads its own php://input, which is what leaves the
// intruder's stream and flag standing where this request will look.

$t = new TestCase('php_input_survives_suspend', 'hooks');

// Sent by the suite line for this test; a literal here rather than a read of the
// request object, so the two sources are independent.
$own = '{"who":"outer","marker":"outer-body-4a71"}';

$before = file_get_contents('php://input');
$t->assertSame('php://input is this request body before the suspend', $before, $own);

$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue('inner self-request socket connected', $sock !== false);
stream_set_timeout($sock, 5);

$innerBody = '{"who":"intruder","marker":"intruder-body-8d21"}';
fwrite($sock, "POST /tests/hooks/fixture_inner_input.php HTTP/1.0\r\n"
    . "Host: 127.0.0.1\r\n"
    . "Content-Type: application/json\r\n"
    . "Content-Length: " . strlen($innerBody) . "\r\n"
    . "Connection: close\r\n\r\n"
    . $innerBody);

sleep(2);                                   // hooked: suspends this request fiber

$resp = (string) stream_get_contents($sock);
fclose($sock);

// Without these two the rest proves nothing: if the intruder never ran inside
// the suspended window, or ran without reading php://input, this request's SAPI
// post state was never at risk in the first place.
$t->assertContains('intruder was served while this request was parked', $resp, 'INNER-OK');
$t->assertContains('intruder read its own body from php://input', $resp, 'intruder-body-8d21');

$after = file_get_contents('php://input');

$t->assertSame('php://input still is this request body after the suspend', $after, $own);
$t->assertSame('php://input reads the same before and after the suspend', $after, $before);

// This is the value the intruder would have left behind, so a failure here says
// what leaked.
$t->assertNotContains('php://input did not pick up the intruder\'s body',
    $after, 'intruder-body');

// The two views of one request's body must agree. The request object carries its
// body across a suspension already, so a php://input that does not says two
// different things about the request being served.
$t->assertSame('php://input and the request object agree after the suspend',
    $after, oxphp_http_request()->body());

$t->done();
