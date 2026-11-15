<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';

$t = new TestCase('curl_easy_in_a_shared_multi_waits_for_its_removal', 'hooks');

// A transfer in a multi handle is under way between the calls on the multi handle
// as well as inside them, and the request driving it can be suspended between two
// of them: here in usleep(), as an HTTP client is while it runs its promise
// callbacks, which leaves the multi handle unclaimed for that moment. A second
// request's curl_setopt() on the easy handle in that transfer waits until the
// first request has taken the handle out — libcurl leaves changing the options of
// a transfer under way undefined, and for a CURLOPT_POSTFIELDS body that is a
// use-after-free.
$holder = curl_hold_start('mode=multi&delay=1500&yield=1');

$parked = curl_hold_parked($sharedState) && isset($sharedState['curl_ch']);
$t->assertTrue('the holding request reached its curl_multi_select() loop and is parked in it', $parked);
if (!$parked) {
    $t->done();
}

$ch = $sharedState['curl_ch'];
$set = null;
try {
    $set = curl_setopt($ch, CURLOPT_USERAGENT, 'second');
} catch (\Throwable $e) {
    $set = get_class($e) . ': ' . $e->getMessage();
}
$returned = microtime(true);

$held = oxphp_async_await($holder, 12.0);
$removedAt = $held['removed_at'] ?? INF;

$t->assertSame('curl_setopt() succeeded', $set, true);
$t->assertSame("the holder's transfer got its whole answer", $held['body'] ?? $held, 'curl-done:/1500/holder');
$t->assertSame("and the holder took its handle out", $held['removed'] ?? null, CURLM_OK);
$t->assertTrue(
    sprintf('curl_setopt() returned only once the holder had taken the handle out (%.3f s after it)', $returned - $removedAt),
    $returned >= $removedAt
);
$t->assertTrue('and soon after it, not at the end of a bound', $returned - $removedAt < 0.5);

unset($sharedState['curl_ch'], $sharedState['curl_mh'], $sharedState['curl_parked']);

$t->done();
