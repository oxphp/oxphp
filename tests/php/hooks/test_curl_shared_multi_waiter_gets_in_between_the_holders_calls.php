<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';

$t = new TestCase('curl_shared_multi_waiter_gets_in_between_the_holders_calls', 'hooks');

// The first request runs a curl_multi_exec()/curl_multi_select() loop on a shared
// multi handle for 4 s, suspending only inside curl_multi_select(), which holds the
// handle. A call from a second request gets the handle between two of the loop's
// calls: it waits for the call in progress, which it ends when that is a
// curl_multi_select(), and not for the whole loop, which would also turn into
// AsyncException once the loop outlasted the wait's bound.
$peer = net_peer_ip();
$holder = curl_hold_start('mode=multi&delay=4000');

$parked = curl_hold_parked($sharedState) && isset($sharedState['curl_mh']);
$t->assertTrue('the holding request reached its curl_multi_select() loop and is parked in it', $parked);
if (!$parked) {
    $t->done();
}

$mh = $sharedState['curl_mh'];
$ch = curl_init("http://{$peer}:8080/100/second");
curl_setopt_array($ch, [CURLOPT_RETURNTRANSFER => true, CURLOPT_TIMEOUT => 8]);
$started = microtime(true);
$added = null;
try {
    $added = curl_multi_add_handle($mh, $ch);
} catch (\Throwable $e) {
    $added = get_class($e) . ': ' . $e->getMessage();
}
$addedAt = microtime(true);

$codes = [];
$deadline = microtime(true) + 8.0;
while (is_int($added) && microtime(true) < $deadline) {
    $codes[curl_multi_exec($mh, $running)] = true;
    if (curl_multi_getcontent($ch) === 'curl-done:/100/second') {
        break;
    }
    curl_multi_select($mh, 0.5);
}
$body = curl_multi_getcontent($ch);
$removed = is_int($added) ? curl_multi_remove_handle($mh, $ch) : null;

$held = oxphp_async_await($holder, 12.0);

$t->assertSame('curl_multi_add_handle() succeeded', $added, CURLM_OK);
$t->assertTrue(sprintf('and returned long before the loop ended (%.3f s)', $addedAt - $started),
    $addedAt - $started <= 1.5);
$t->assertTrue(sprintf("while the holder's loop was still running (it ended %.3f s later)",
        ($held['finished'] ?? -INF) - $addedAt),
    ($held['finished'] ?? -INF) - $addedAt >= 1.0);
$t->assertSame("every one of the second request's curl_multi_exec() calls succeeded", array_keys($codes), [CURLM_OK]);
$t->assertSame("the second request's transfer completed", $body, 'curl-done:/100/second');
$t->assertSame('curl_multi_remove_handle() succeeded', $removed, CURLM_OK);
$t->assertSame("the holder's transfer got its whole answer", $held['body'] ?? $held, 'curl-done:/4000/holder');
$t->assertSame("and every one of the holder's curl_multi_exec() calls succeeded", $held['codes'] ?? null, [CURLM_OK]);

unset($sharedState['curl_mh'], $sharedState['curl_ch'], $sharedState['curl_parked']);

$t->done();
