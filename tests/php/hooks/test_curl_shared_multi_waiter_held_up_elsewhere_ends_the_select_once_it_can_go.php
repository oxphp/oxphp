<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';

$t = new TestCase('curl_shared_multi_waiter_held_up_elsewhere_ends_the_select_once_it_can_go', 'hooks');

// A call waiting for a multi handle another request's loop holds in a 5 s
// curl_multi_select(), and for an easy handle a third request is running
// curl_exec() on for 2 s. It ends the loop's curl_multi_select() when it can go
// ahead, once curl_exec() is over, and not before: ended while the easy handle is
// still held, the loop could not let it in and would go into its next 5 s select,
// which the call would then wait out.
unset($sharedState['curl_mh']);
$easy = curl_hold_start('mode=easy&delay=2000');
$parked = curl_hold_parked($sharedState) && isset($sharedState['curl_ch']);
$t->assertTrue('the first request is parked in curl_exec()', $parked);
if (!$parked) {
    $t->done();
}
$ch = $sharedState['curl_ch'];

$holder = curl_hold_start('mode=multi&delay=6000&select=5');
$inLongSelect = static fn (array $s): bool => isset($s['curl_mh']) && ($s['curl_in_select'] ?? false)
    && microtime(true) - ($s['curl_select_started'] ?? INF) >= 0.2;
$deadline = microtime(true) + 3.0;
while (!$inLongSelect($sharedState) && microtime(true) < $deadline) {
    oxphp_usleep(20_000);
}
$ready = $inLongSelect($sharedState);
$t->assertTrue("the second request is parked in its loop's curl_multi_select()", $ready);
if (!$ready) {
    $t->done();
}
$mh = $sharedState['curl_mh'];

$started = microtime(true);
try {
    $added = curl_multi_add_handle($mh, $ch);
} catch (\Throwable $e) {
    $added = get_class($e) . ': ' . $e->getMessage();
}
$addedAt = microtime(true);
$removed = $added === CURLM_OK ? curl_multi_remove_handle($mh, $ch) : null;

$ran = oxphp_async_await($easy, 12.0);
$held = oxphp_async_await($holder, 12.0);
$execEnded = $ran['finished'] ?? INF;

// The loop's curl_multi_select() under way when the call was made.
$cut = null;
foreach ($held['selects'] ?? [] as [$from, $to, $returned]) {
    if ($from <= $started && $to >= $started) {
        $cut = [$to - $execEnded, $returned];
    }
}

$t->assertTrue(sprintf('curl_exec() was still running when the call was made (it ended %.3f s later)',
        $execEnded - $started), $execEnded > $started);
$t->assertSame('curl_multi_add_handle() succeeded', $added, CURLM_OK);
$t->assertTrue(sprintf('once curl_exec() was over, and promptly (%.3f s after it)', $addedAt - $execEnded),
    $addedAt >= $execEnded && $addedAt - $execEnded <= 0.3);
$t->assertTrue(sprintf("the loop's curl_multi_select() under way at the call was the one that ended, "
        . 'returning 0 within 0.3 s of curl_exec() (%s)', $cut === null ? 'none found'
        : sprintf('%.3f s after it, returned %s', $cut[0], var_export($cut[1], true))),
    $cut !== null && $cut[1] === 0 && $cut[0] >= 0 && $cut[0] <= 0.3);
$t->assertSame('curl_multi_remove_handle() succeeded', $removed, CURLM_OK);
$t->assertSame("the first request's transfer got its whole answer", $ran['body'] ?? $ran, 'curl-done:/2000/holder');
$t->assertSame("the loop's transfer got its whole answer", $held['body'] ?? $held, 'curl-done:/6000/holder');
$t->assertSame("and every one of the loop's curl_multi_exec() calls succeeded", $held['codes'] ?? null, [CURLM_OK]);

unset($sharedState['curl_mh'], $sharedState['curl_ch'], $sharedState['curl_parked'],
    $sharedState['curl_in_select'], $sharedState['curl_select_started']);

$t->done();
