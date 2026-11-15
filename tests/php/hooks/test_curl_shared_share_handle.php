<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';

$t = new TestCase('curl_shared_share_handle', 'hooks');

// One CurlShareHandle — DNS cache and connection pool — under two transfers on
// different CurlHandles, both in flight at once on one worker: the first parked in
// curl_exec() for two seconds, the second run from start to finish meanwhile.
// Share handles are not held for the length of a call the way easy and multi
// handles are, so the second transfer does not wait, and both have to come out
// whole.
$peer = net_peer_ip();
$holder = curl_hold_start('mode=share&delay=2000');

$parked = curl_hold_parked($sharedState) && isset($sharedState['curl_sh']);
$t->assertTrue('the holding request reached its curl_exec() and is parked in it', $parked);
if (!$parked) {
    $t->done();
}

$ch = curl_init("http://{$peer}:8080/300/second");
curl_setopt_array($ch, [
    CURLOPT_RETURNTRANSFER => true,
    CURLOPT_TIMEOUT => 8,
    CURLOPT_SHARE => $sharedState['curl_sh'],
]);
$body = curl_exec($ch);
$errno = curl_errno($ch);
$finished = microtime(true);

$held = oxphp_async_await($holder, 12.0);

$t->assertSame("the second transfer, on the holder's share handle, got its answer", $body, 'curl-done:/300/second');
$t->assertSame('without an error', $errno, 0);
$t->assertTrue(
    "and finished while the holder's transfer was still waiting",
    isset($held['finished']) && $finished < $held['finished']
);
$t->assertSame("the holder's transfer got its whole answer", $held['body'] ?? $held, 'curl-done:/2000/holder');
$t->assertSame('and ended without an error', $held['errno'] ?? null, 0);

unset($sharedState['curl_ch'], $sharedState['curl_sh'], $sharedState['curl_parked']);

$t->done();
