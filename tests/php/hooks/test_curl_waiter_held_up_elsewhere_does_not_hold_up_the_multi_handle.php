<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';

$t = new TestCase('curl_waiter_held_up_elsewhere_does_not_hold_up_the_multi_handle', 'hooks');

// A call waiting for a multi handle gets it between the holder's calls only when it
// can go ahead then. This one also needs an easy handle a third request is running
// curl_exec() on for 3 s, so the holder's 1 s loop on the multi handle carries on
// meanwhile instead of standing aside for a call that could not use its turn.
unset($sharedState['curl_mh']);
$easy = curl_hold_start('mode=easy&delay=3000');
$parked = curl_hold_parked($sharedState) && isset($sharedState['curl_ch']);
$t->assertTrue('the first request is parked in curl_exec()', $parked);
if (!$parked) {
    $t->done();
}
$ch = $sharedState['curl_ch'];

$looping = microtime(true);
$holder = curl_hold_start('mode=multi&delay=1000');
$deadline = microtime(true) + 3.0;
while (!isset($sharedState['curl_mh']) && microtime(true) < $deadline) {
    oxphp_usleep(20_000);
}
$t->assertTrue('the second request started its curl_multi_select() loop', isset($sharedState['curl_mh']));
if (!isset($sharedState['curl_mh'])) {
    $t->done();
}
oxphp_usleep(200_000);
$mh = $sharedState['curl_mh'];

$added = null;
try {
    $added = curl_multi_add_handle($mh, $ch);
} catch (\Throwable $e) {
    $added = get_class($e) . ': ' . $e->getMessage();
}
$addedAt = microtime(true);
$removed = is_int($added) ? curl_multi_remove_handle($mh, $ch) : null;

$held = oxphp_async_await($holder, 12.0);
$ran = oxphp_async_await($easy, 12.0);

$t->assertSame("the first request's transfer got its whole answer", $ran['body'] ?? $ran, 'curl-done:/3000/holder');
$t->assertSame('curl_multi_add_handle() succeeded', $added, CURLM_OK);
$t->assertTrue(sprintf('once curl_exec() was over (%.3f s after it)', $addedAt - ($ran['finished'] ?? INF)),
    $addedAt >= ($ran['finished'] ?? INF));
$t->assertSame('curl_multi_remove_handle() succeeded', $removed, CURLM_OK);
$t->assertSame("the loop's transfer got its whole answer", $held['body'] ?? $held, 'curl-done:/1000/holder');
$t->assertTrue(sprintf('and the loop ended %.3f s after it began, not held up by the waiting call',
        ($held['finished'] ?? INF) - $looping),
    ($held['finished'] ?? INF) - $looping < 2.2);

unset($sharedState['curl_mh'], $sharedState['curl_ch'], $sharedState['curl_parked']);

$t->done();
