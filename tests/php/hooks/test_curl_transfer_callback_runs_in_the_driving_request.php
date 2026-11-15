<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';

$t = new TestCase('curl_transfer_callback_runs_in_the_driving_request', 'hooks');

// A transfer's callbacks run in whichever request drives its multi handle. The
// first request adds a handle whose progress callback calls curl_setopt() on it,
// then waits without driving the multi handle; this request drives it. The
// callback's call comes from inside this request's curl_multi_exec(), which
// libcurl allows, and is not held back for the first request, which keeps the
// handle until it takes it out — that would be a wait neither side could end.
unset($sharedState['curl_release'], $sharedState['curl_cb_took']);
$holder = curl_hold_start('mode=member&how=wait&cb=1&delay=100');

$parked = curl_hold_parked($sharedState) && isset($sharedState['curl_mh'], $sharedState['curl_ch']);
$t->assertTrue('the first request added its handle and is waiting', $parked);
if (!$parked) {
    $t->done();
}

$mh = $sharedState['curl_mh'];
$ch = $sharedState['curl_ch'];
$codes = [];
$threw = null;
try {
    $deadline = microtime(true) + 8.0;
    while (microtime(true) < $deadline) {
        $codes[curl_multi_exec($mh, $running)] = true;
        if ($running === 0) {
            break;
        }
        curl_multi_select($mh, 0.5);
    }
} catch (\Throwable $e) {
    $threw = get_class($e) . ': ' . $e->getMessage();
}
$body = curl_multi_getcontent($ch);
$holderWaiting = (bool) ($sharedState['curl_parked'] ?? false);
$took = $sharedState['curl_cb_took'] ?? null;

$sharedState['curl_release'] = true;
$held = oxphp_async_await($holder, 12.0);

$t->assertSame('driving the multi handle threw nothing', $threw, null);
$t->assertSame('every curl_multi_exec() call succeeded', array_keys($codes), [CURLM_OK]);
$t->assertSame("the first request's transfer completed here", $body, 'curl-done:/100/holder');
$t->assertTrue('the first request was still waiting while it did', $holderWaiting);
$t->assertTrue('the progress callback ran in this request and called curl_setopt()', $took !== null);
$t->assertTrue(sprintf("that call returned at once (%.3f s)", (float) $took), $took !== null && $took < 0.2);
$t->assertSame('the first request then took its handle out', $held['removed'] ?? $held, CURLM_OK);

unset($sharedState['curl_ch'], $sharedState['curl_mh'], $sharedState['curl_parked'],
    $sharedState['curl_release'], $sharedState['curl_cb_took']);

$t->done();
