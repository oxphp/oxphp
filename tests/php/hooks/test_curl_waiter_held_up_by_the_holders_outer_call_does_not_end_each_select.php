<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';

$t = new TestCase('curl_waiter_held_up_by_the_holders_outer_call_does_not_end_each_select', 'hooks');

// Another request runs curl_exec() for 4 s, and the transfer's progress callback
// runs a curl_multi_exec()/curl_multi_select() loop on a multi handle of its own for
// 2 s. A call that needs both that multi handle and the easy handle,
// curl_multi_add_handle() here, finds both held by one fiber that is in a
// curl_multi_select(), as if the select were all that kept it waiting; but the loop
// cannot let it in, since curl_exec() holds the easy handle until it returns. The
// call may end that select, not each one after it: the loop would go round once
// every few milliseconds for as long as the call waits.
unset($sharedState['curl_mh']);
$holder = curl_hold_start('mode=easy&delay=4000&inner=multi');
$inLongSelect = static fn (array $s): bool => isset($s['curl_mh']) && ($s['curl_in_select'] ?? false)
    && microtime(true) - ($s['curl_select_started'] ?? INF) >= 0.2;
$deadline = microtime(true) + 3.0;
while (!$inLongSelect($sharedState) && microtime(true) < $deadline) {
    oxphp_usleep(20_000);
}
$ready = $inLongSelect($sharedState) && isset($sharedState['curl_ch']);
$t->assertTrue("the other request is parked in its progress callback's curl_multi_select()", $ready);
if (!$ready) {
    $t->done();
}
$mh = $sharedState['curl_mh'];
$ch = $sharedState['curl_ch'];

$started = microtime(true);
try {
    $added = curl_multi_add_handle($mh, $ch);
} catch (\Throwable $e) {
    $added = get_class($e) . ': ' . $e->getMessage();
}
$addedAt = microtime(true);
$removed = $added === CURLM_OK ? curl_multi_remove_handle($mh, $ch) : null;

$held = oxphp_async_await($holder, 12.0);
$execEnded = $held['finished'] ?? INF;
$selects = count($held['selects'] ?? []);

$t->assertTrue(sprintf('curl_exec() was still running when the call was made (it ended %.3f s later)',
        $execEnded - $started), $execEnded > $started);
$t->assertSame('curl_multi_add_handle() succeeded', $added, CURLM_OK);
$t->assertTrue(sprintf('once curl_exec() was over (%.3f s after it)', $addedAt - $execEnded), $addedAt >= $execEnded);
$t->assertTrue(sprintf("the callback's loop made a handful of curl_multi_select() calls over 2 s (%d)", $selects),
    $selects >= 1 && $selects <= 20);
$t->assertSame("the callback's transfer got its whole answer", $held['inner_body'] ?? $held, 'curl-done:/2000/inner');
$t->assertSame("curl_exec()'s transfer got its whole answer", $held['body'] ?? $held, 'curl-done:/4000/holder');
$t->assertSame('curl_multi_remove_handle() succeeded', $removed, CURLM_OK);

unset($sharedState['curl_mh'], $sharedState['curl_ch'], $sharedState['curl_parked'],
    $sharedState['curl_in_select'], $sharedState['curl_select_started']);

$t->done();
