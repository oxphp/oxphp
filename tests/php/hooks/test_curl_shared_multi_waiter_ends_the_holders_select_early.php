<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';

$t = new TestCase('curl_shared_multi_waiter_ends_the_holders_select_early', 'hooks');

// The first request runs a curl_multi_exec()/curl_multi_select() loop on a shared
// multi handle, passing curl_multi_select() 5 s, for a transfer answered after 6 s.
// A call from a second request that is not one of a loop's own calls ends that
// curl_multi_select() as its timeout would, so it gets the handle at once instead
// of when the select returns.
$peer = net_peer_ip();
$holder = curl_hold_start('mode=multi&delay=6000&select=5');

// Parked in the 5 s select, not in one of the short waits libcurl makes while it
// connects: the premise is read at the moment of the first call.
$inLongSelect = static fn (array $s): bool => ($s['curl_in_select'] ?? false)
    && microtime(true) - ($s['curl_select_started'] ?? INF) >= 0.2;
$parked = curl_hold_parked($sharedState) && isset($sharedState['curl_mh']);
$deadline = microtime(true) + 2.0;
while ($parked && !$inLongSelect($sharedState) && microtime(true) < $deadline) {
    oxphp_usleep(20_000);
}
$ready = $parked && $inLongSelect($sharedState);
$t->assertTrue("the holding request is parked in its loop's curl_multi_select()", $ready);
if (!$ready) {
    $t->done();
}

$mh = $sharedState['curl_mh'];
$ch = curl_init("http://{$peer}:8080/100/second");
curl_setopt_array($ch, [CURLOPT_RETURNTRANSFER => true, CURLOPT_TIMEOUT => 8]);
$started = microtime(true);
try {
    $added = curl_multi_add_handle($mh, $ch);
} catch (\Throwable $e) {
    $added = get_class($e) . ': ' . $e->getMessage();
}
$addedAt = microtime(true);

$codes = [];
$deadline = microtime(true) + 8.0;
while ($added === CURLM_OK && microtime(true) < $deadline) {
    $codes[curl_multi_exec($mh, $running)] = true;
    if (curl_multi_getcontent($ch) === 'curl-done:/100/second') {
        break;
    }
    curl_multi_select($mh, 0.5);
}
$body = curl_multi_getcontent($ch);
$removed = $added === CURLM_OK ? curl_multi_remove_handle($mh, $ch) : null;

$held = oxphp_async_await($holder, 12.0);

// The holder's select that was under way when the call was made.
$cut = null;
foreach ($held['selects'] ?? [] as [$from, $to, $returned]) {
    if ($from <= $started && $to >= $started) {
        $cut = [$to - $started, $returned];
    }
}

$t->assertSame('curl_multi_add_handle() succeeded', $added, CURLM_OK);
$t->assertTrue(sprintf('and returned at once (%.3f s)', $addedAt - $started), $addedAt - $started <= 0.3);
$t->assertTrue(sprintf("the holder's curl_multi_select() under way at the call returned 0 within 0.3 s "
        . 'of it (%s)', $cut === null ? 'none found'
        : sprintf('%.3f s, returned %s', $cut[0], var_export($cut[1], true))),
    $cut !== null && $cut[1] === 0 && $cut[0] <= 0.3);
$t->assertTrue(sprintf("while the holder's loop went on (it ended %.3f s later)",
        ($held['finished'] ?? -INF) - $addedAt),
    ($held['finished'] ?? -INF) - $addedAt >= 3.0);
$t->assertSame("every one of the second request's curl_multi_exec() calls succeeded", array_keys($codes), [CURLM_OK]);
$t->assertSame("the second request's transfer completed", $body, 'curl-done:/100/second');
$t->assertSame('curl_multi_remove_handle() succeeded', $removed, CURLM_OK);
$t->assertSame("the holder's transfer got its whole answer", $held['body'] ?? $held, 'curl-done:/6000/holder');
$t->assertSame("and every one of the holder's curl_multi_exec() calls succeeded", $held['codes'] ?? null, [CURLM_OK]);

unset($sharedState['curl_mh'], $sharedState['curl_ch'], $sharedState['curl_parked'],
    $sharedState['curl_in_select'], $sharedState['curl_select_started']);

$t->done();
