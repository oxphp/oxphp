<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';

$t = new TestCase('curl_shared_handle_waits_for_the_holder', 'hooks');

// One CurlHandle, two requests: the first is parked inside curl_exec() on it,
// waiting for an answer two seconds away, and the second changes the handle's URL
// and runs a transfer of its own on it — what a handle kept in a static or in the
// worker's container gets as soon as two requests overlap. Neither call may run
// inside the first one's transfer: libcurl refuses a second perform on a handle
// that is still performing, ext/curl empties the first transfer's
// CURLOPT_RETURNTRANSFER buffer before it gets that far, and a new URL becomes
// the one the parked transfer reports as its own. So the second request waits
// until the first call has returned, and then gets the handle to itself.
$peer = net_peer_ip();
$holder = curl_hold_start('mode=easy&delay=2000');

$parked = curl_hold_parked($sharedState) && isset($sharedState['curl_ch']);
$t->assertTrue('the holding request reached its curl_exec() and is parked in it', $parked);
if (!$parked) {
    $t->done();
}

$ch = $sharedState['curl_ch'];
curl_setopt($ch, CURLOPT_URL, "http://{$peer}:8080/100/second");
$setoptReturned = microtime(true);
$body = curl_exec($ch);
$errno = curl_errno($ch);

$held = oxphp_async_await($holder, 12.0);

$t->assertSame("the holder's transfer got its whole answer", $held['body'] ?? $held, 'curl-done:/2000/holder');
$t->assertSame('and ended without an error', $held['errno'] ?? null, 0);
$t->assertTrue(
    "the second request's curl_setopt() returned only after the holder's curl_exec() had",
    isset($held['finished']) && $setoptReturned >= $held['finished']
);
$t->assertSame("the second request's own transfer then ran on the handle", $body, 'curl-done:/100/second');
$t->assertSame('without an error', $errno, 0);

unset($sharedState['curl_ch'], $sharedState['curl_parked']);

$t->done();
