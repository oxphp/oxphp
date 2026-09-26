<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A php://input handle a request opened before it suspended goes on reading that
// request's body after it resumes.
//
// The end of every request closes the php://input handles still open on its
// body, because the body goes away with the request and a handle left standing
// would read freed memory. What it must not close is a handle on somebody
// else's body — and on a worker, the request that ends is often not the only one
// live on it: while this request is parked, the inner self-request below runs to
// completion on the same worker and ends there. A worker that closed every
// php://input handle it found at that point, rather than the ones on the ending
// request's own body, would close this one out from under it.
//
// The two sibling tests read php://input with file_get_contents(), which opens
// and closes a handle of its own on each call and so holds none across the
// window. This one keeps its handle open through the suspension and reads the
// rest of the body through it afterwards.
//
// PHP_WORKERS=1, so the inner self-request can only be served while this fiber
// is suspended.

$t = new TestCase('php_input_handle_survives_suspend', 'hooks');

// Sent by the suite line for this test; kept in step with it by hand.
$own = '{"who":"outer","marker":"outer-body-7e19"}';

$h = fopen('php://input', 'r');
$t->assertTrue('php://input opened before the suspend', is_resource($h));

// Unbuffered, or the first fread() below pulls the whole body into the handle's
// own read buffer and every read after the suspend is served from that copy —
// which would still pass with the handle pointing at the wrong body or at a
// freed one. With no buffer, each read goes through the wrapper to the body it
// holds, from the position it has reached.
$t->assertSame('the handle can be made unbuffered', stream_set_read_buffer($h, 0), 0);

// Half before, half after: the position is part of what the handle carries
// across the window, so reading from the start again afterwards would not tell
// a surviving handle from a reopened one.
$half = intdiv(strlen($own), 2);
$head = (string) fread($h, $half);
$t->assertSame('the first half reads before the suspend', $head, substr($own, 0, $half));

$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue('inner self-request socket connected', $sock !== false);
stream_set_timeout($sock, 5);

$innerBody = '{"who":"intruder","marker":"intruder-body-3f82"}';
fwrite($sock, "POST /tests/hooks/fixture_inner_input.php HTTP/1.0\r\n"
    . "Host: 127.0.0.1\r\n"
    . "Content-Type: application/json\r\n"
    . "Content-Length: " . strlen($innerBody) . "\r\n"
    . "Connection: close\r\n\r\n"
    . $innerBody);

sleep(2);                                   // hooked: suspends this request fiber

$resp = (string) stream_get_contents($sock);
fclose($sock);

// The window has to have held a request that ended with a body of its own, or
// the end-of-request close never ran while this handle was open.
$t->assertContains('intruder was served while this request was parked', $resp, 'INNER-OK');
$t->assertContains('intruder read its own body from php://input', $resp, 'intruder-body-3f82');

// Asked before reading: on a worker that closed this handle, fread() would raise
// a TypeError and end the test with a less legible message.
$t->assertTrue('the handle is still open after the suspend', is_resource($h));

if (is_resource($h)) {
    $tail = (string) stream_get_contents($h);
    $t->assertSame('the rest of this request body reads through the same handle',
        $head . $tail, $own);
    $t->assertNotContains('the handle did not pick up the intruder\'s body',
        $tail, 'intruder-body');
    fclose($h);
}

$t->done();
