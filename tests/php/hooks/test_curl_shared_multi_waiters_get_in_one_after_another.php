<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';

$t = new TestCase('curl_shared_multi_waiters_get_in_one_after_another', 'hooks');

// Two requests wait for a shared multi handle that a third one's 4 s loop holds.
// Both get it between the loop's calls, in the order they came, long before the
// loop ends: neither stands aside for the other.
$peer = net_peer_ip();
unset($sharedState['curl_adder_waiting']);
$holder = curl_hold_start('mode=multi&delay=4000');

$parked = curl_hold_parked($sharedState) && isset($sharedState['curl_mh']);
$t->assertTrue('the holding request reached its curl_multi_select() loop and is parked in it', $parked);
if (!$parked) {
    $t->done();
}

$adder = curl_hold_start('mode=adder&delay=100');
$deadline = microtime(true) + 3.0;
while (!isset($sharedState['curl_adder_waiting']) && microtime(true) < $deadline) {
    oxphp_usleep(5_000);
}
$t->assertTrue('the second request is waiting to add its transfer', isset($sharedState['curl_adder_waiting']));
if (!isset($sharedState['curl_adder_waiting'])) {
    $t->done();
}

$adderStarted = $sharedState['curl_adder_waiting'];
$mh = $sharedState['curl_mh'];
$ch = curl_init("http://{$peer}:8080/100/third");
curl_setopt_array($ch, [CURLOPT_RETURNTRANSFER => true, CURLOPT_TIMEOUT => 8]);
$started = microtime(true);
$added = null;
try {
    $added = curl_multi_add_handle($mh, $ch);
} catch (\Throwable $e) {
    $added = get_class($e) . ': ' . $e->getMessage();
}
$took = microtime(true) - $started;

$deadline = microtime(true) + 8.0;
while (is_int($added) && microtime(true) < $deadline) {
    curl_multi_exec($mh, $running);
    if (curl_multi_getcontent($ch) === 'curl-done:/100/third') {
        break;
    }
    curl_multi_select($mh, 0.5);
}
$body = curl_multi_getcontent($ch);
$removed = is_int($added) ? curl_multi_remove_handle($mh, $ch) : null;

$second = oxphp_async_await($adder, 12.0);
$held = oxphp_async_await($holder, 12.0);

$t->assertSame("the second request's curl_multi_add_handle() succeeded", $second['added'] ?? $second, CURLM_OK);
$t->assertTrue(sprintf('within %.3f s', $second['add_took'] ?? INF), ($second['add_took'] ?? INF) <= 2.5);
$t->assertSame("this request's curl_multi_add_handle() succeeded", $added, CURLM_OK);
$t->assertTrue(sprintf('within %.3f s', $took), $took <= 2.5);
$t->assertTrue(sprintf('after the second request, which came first (%.3f s after it)',
        ($started + $took) - ($adderStarted + ($second['add_took'] ?? INF))),
    $adderStarted + ($second['add_took'] ?? INF) <= $started + $took);
$t->assertTrue(sprintf("both while the holder's loop was still running (it ended %.3f s after the second one)",
        ($held['finished'] ?? -INF) - ($started + $took)),
    ($held['finished'] ?? -INF) - ($started + $took) >= 0.5);
$t->assertSame("the second request's transfer completed", $second['body'] ?? null, 'curl-done:/100/adder');
$t->assertSame("this request's transfer completed", $body, 'curl-done:/100/third');
$t->assertSame('curl_multi_remove_handle() succeeded', $removed, CURLM_OK);
$t->assertSame("the holder's transfer got its whole answer", $held['body'] ?? $held, 'curl-done:/4000/holder');

unset($sharedState['curl_mh'], $sharedState['curl_ch'], $sharedState['curl_parked'],
    $sharedState['curl_adder_waiting']);

$t->done();
