<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';

$t = new TestCase('curl_easy_waiter_is_not_held_by_the_next_handle_added', 'hooks');

// A call waiting for an easy handle to come out of another request's multi handle
// goes ahead once that handle is out, even when the other request puts a new handle
// into the same multi handle straight after, without suspending in between, and
// keeps it there. Waiting on for the new handle would hold the call for 1.5 s here,
// and for as long as the other request keeps adding transfers in a real client.
unset($sharedState['curl_release']);
$holder = curl_hold_start('mode=member&how=wait&readd=1&delay=100');

$ready = curl_hold_parked($sharedState) && isset($sharedState['curl_ch']);
$t->assertTrue('the first request added its handle and is waiting', $ready);
if (!$ready) {
    $sharedState['curl_release'] = true;
    $t->done();
}

$ch = $sharedState['curl_ch'];
$holderWaiting = (bool) ($sharedState['curl_parked'] ?? false);
// Released only now, so the first request takes the handle out after the call
// below has started waiting for it.
$sharedState['curl_release'] = true;
$set = null;
try {
    $set = curl_setopt($ch, CURLOPT_USERAGENT, 'second');
} catch (\Throwable $e) {
    $set = get_class($e) . ': ' . $e->getMessage();
}
$returned = microtime(true);
$held = oxphp_async_await($holder, 12.0);

$t->assertTrue('the first request still had the handle in when the call started', $holderWaiting);
$t->assertSame('curl_setopt() succeeded', $set, true);
$t->assertSame('the first request took its handle out', $held['removed'] ?? $held, CURLM_OK);
$t->assertSame('and put a new one in at once', $held['readded'] ?? $held, CURLM_OK);
$after = $returned - ($held['removed_at'] ?? INF);
$t->assertTrue(sprintf('curl_setopt() returned after the handle came out (%.3f s after it)', $after), $after >= 0);
$t->assertTrue(
    sprintf('and did not wait for the new handle, out %.3f s after it returned',
        ($held['readded_out_at'] ?? INF) - $returned),
    $returned < ($held['readded_out_at'] ?? -INF) - 0.5
);

unset($sharedState['curl_ch'], $sharedState['curl_mh'], $sharedState['curl_parked'],
    $sharedState['curl_release']);

$t->done();
