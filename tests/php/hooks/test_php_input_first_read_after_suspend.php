<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// The other half of hooks/test_php_input_survives_suspend: a request that never
// touches php://input before it suspends must not find someone else's body
// there afterwards.
//
// Nothing of this request's is standing in the thread-wide SAPI post state when
// it parks — it read no body, so there is no stream of its own and no flag set.
// What it must come back to is that same emptiness, which is what makes the SAPI
// hand it its own bytes on the first read. Come back to what the request served
// in the window left instead, and the first read of php://input in a request
// that has read nothing returns another client's body in full.
//
// PHP_WORKERS=1, so the inner self-request below can only be served while this
// fiber is suspended.

$t = new TestCase('php_input_first_read_after_suspend', 'hooks');

// Sent by the suite line for this test. Deliberately not read before the
// suspension — that is the whole point of this case.
$own = '{"who":"outer","marker":"outer-body-2b60"}';

$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue('inner self-request socket connected', $sock !== false);
stream_set_timeout($sock, 5);

$innerBody = '{"who":"intruder","marker":"intruder-body-5c47"}';
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
$t->assertContains('intruder read its own body from php://input', $resp, 'intruder-body-5c47');

$first = file_get_contents('php://input');

$t->assertSame('the first read of php://input after the suspend is this request body',
    $first, $own);
$t->assertNotContains('the first read did not pick up the intruder\'s body',
    $first, 'intruder-body');

// And it stays readable afterwards, the way php://input is for any request that
// has a buffered body.
$t->assertSame('php://input is still re-readable after that first read',
    file_get_contents('php://input'), $own);

$t->done();
