<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';

$t = new TestCase('curl_nested_call_keeps_the_handle', 'hooks');

// The holder's transfer calls curl_setopt() on its own handle from its progress
// callback, from inside curl_exec(), and then waits two seconds for its answer.
// That nested call is the holder's own and must not have to wait for itself; and
// when it returns, the handle is still the holder's, because the curl_exec()
// around it has not. So a second request's curl_setopt() on the handle waits
// until the holder's curl_exec() has returned, exactly as it would without the
// nested call.
$peer = net_peer_ip();
unset($sharedState['curl_nested']);
$holder = curl_hold_start('mode=easy&delay=2000&nested=1');

$parked = curl_hold_parked($sharedState) && isset($sharedState['curl_ch']);
$t->assertTrue('the holding request reached its curl_exec() and is parked in it', $parked);
$t->assertTrue(
    "premise: the holder's callback already made its nested call on the handle",
    ($sharedState['curl_nested'] ?? false) === true
);
if (!$parked) {
    $t->done();
}

$ch = $sharedState['curl_ch'];
curl_setopt($ch, CURLOPT_URL, "http://{$peer}:8080/100/second");
$setoptReturned = microtime(true);

$held = oxphp_async_await($holder, 12.0);

$t->assertSame("the holder's transfer got its whole answer", $held['body'] ?? $held, 'curl-done:/2000/holder');
$t->assertSame('and ended without an error', $held['errno'] ?? null, 0);
$t->assertTrue(
    "the second request's curl_setopt() returned only after the holder's curl_exec() had",
    isset($held['finished']) && $setoptReturned >= $held['finished']
);

unset($sharedState['curl_ch'], $sharedState['curl_parked'], $sharedState['curl_nested']);

$t->done();
