<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';

$t = new TestCase('curl_shared_multi_waits_for_the_holder', 'hooks');

// One CurlMultiHandle, two requests — the shape of an HTTP client that keeps one
// multi handle for the life of the worker. The first is parked in
// curl_multi_select() on it; the second adds a transfer of its own and drives the
// multi handle until that transfer is done. libcurl refuses every call on a multi
// handle while another call on it is still running, with
// CURLM_RECURSIVE_API_CALL, so each of the second request's calls waits until the
// first one's current call has returned; curl_multi_exec() instead returns at once
// and leaves the transfers to whichever request's curl_multi_exec() runs next. Both
// transfers complete, and every call succeeds.
$peer = net_peer_ip();
$holder = curl_hold_start('mode=multi&delay=2000');

$parked = curl_hold_parked($sharedState) && isset($sharedState['curl_mh']);
$t->assertTrue('the holding request reached its curl_multi_select() loop and is parked in it', $parked);
if (!$parked) {
    $t->done();
}

$mh = $sharedState['curl_mh'];
$ch = curl_init("http://{$peer}:8080/100/second");
curl_setopt_array($ch, [CURLOPT_RETURNTRANSFER => true, CURLOPT_TIMEOUT => 8]);
$added = curl_multi_add_handle($mh, $ch);

$codes = [];
$deadline = microtime(true) + 8.0;
while (microtime(true) < $deadline) {
    $codes[curl_multi_exec($mh, $running)] = true;
    if (curl_multi_getcontent($ch) === 'curl-done:/100/second') {
        break;
    }
    curl_multi_select($mh, 0.5);
}
$body = curl_multi_getcontent($ch);
$removed = curl_multi_remove_handle($mh, $ch);

$held = oxphp_async_await($holder, 12.0);

$t->assertSame('curl_multi_add_handle() succeeded', $added, CURLM_OK);
$t->assertSame("every one of the second request's curl_multi_exec() calls succeeded", array_keys($codes), [CURLM_OK]);
$t->assertSame("the second request's transfer completed", $body, 'curl-done:/100/second');
$t->assertSame('curl_multi_remove_handle() succeeded', $removed, CURLM_OK);
$t->assertSame("the holder's transfer got its whole answer", $held['body'] ?? $held, 'curl-done:/2000/holder');
$t->assertSame("and every one of the holder's curl_multi_exec() calls succeeded", $held['codes'] ?? null, [CURLM_OK]);

unset($sharedState['curl_mh'], $sharedState['curl_parked']);

$t->done();
