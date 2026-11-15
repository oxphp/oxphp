<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';

$t = new TestCase('curl_holder_in_a_userland_fiber_is_not_refused_for_a_waiter', 'hooks');

// The holder of a shared multi handle stands aside between its calls for a call
// that waits for the handle. Its curl_multi_exec() calls here run inside a userland
// Fiber, where it cannot be suspended to stand aside, so they go ahead, as they
// would with no one waiting, rather than being refused. The waiting call ends the
// curl_multi_select() it finds the holder in, and the holder's next
// curl_multi_select(), made outside the Fiber, lets it in.
$peer = net_peer_ip();
$holder = curl_hold_start('mode=multi&delay=2000&fiber=1');

$parked = curl_hold_parked($sharedState) && isset($sharedState['curl_mh']);
$t->assertTrue('the holding request reached its curl_multi_select() loop and is parked in it', $parked);
if (!$parked) {
    $t->done();
}

$mh = $sharedState['curl_mh'];
$ch = curl_init("http://{$peer}:8080/100/second");
curl_setopt_array($ch, [CURLOPT_RETURNTRANSFER => true, CURLOPT_TIMEOUT => 8]);
$added = null;
try {
    $added = curl_multi_add_handle($mh, $ch);
} catch (\Throwable $e) {
    $added = get_class($e) . ': ' . $e->getMessage();
}

$deadline = microtime(true) + 8.0;
while (is_int($added) && microtime(true) < $deadline) {
    curl_multi_exec($mh, $running);
    if (curl_multi_getcontent($ch) === 'curl-done:/100/second') {
        break;
    }
    curl_multi_select($mh, 0.5);
}
$body = curl_multi_getcontent($ch);
$removed = is_int($added) ? curl_multi_remove_handle($mh, $ch) : null;

$held = oxphp_async_await($holder, 12.0);

$t->assertSame('the holder was not refused a call', $held['threw'] ?? null, null);
$t->assertSame("and every one of its curl_multi_exec() calls succeeded", $held['codes'] ?? $held, [CURLM_OK]);
$t->assertSame("the holder's transfer got its whole answer", $held['body'] ?? null, 'curl-done:/2000/holder');
$t->assertTrue(sprintf("in a handful of curl_multi_select() calls over 2 s (%d)", count($held['selects'] ?? [])),
    count($held['selects'] ?? []) >= 1 && count($held['selects'] ?? []) <= 20);
$t->assertSame('curl_multi_add_handle() succeeded', $added, CURLM_OK);
$t->assertSame("the second request's transfer completed", $body, 'curl-done:/100/second');
$t->assertSame('curl_multi_remove_handle() succeeded', $removed, CURLM_OK);

unset($sharedState['curl_mh'], $sharedState['curl_ch'], $sharedState['curl_parked']);

$t->done();
