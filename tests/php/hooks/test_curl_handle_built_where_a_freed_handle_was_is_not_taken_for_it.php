<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';

$t = new TestCase('curl_handle_built_where_a_freed_handle_was_is_not_taken_for_it', 'hooks');

// A multi handle being freed releases its easy handles before it is gone itself,
// and a destructor run in between — of an object only the second handle's callback
// holds — can build an easy handle on the number and memory the first one has just
// given back. That is a new handle, not the first one still in its multi handle:
//
//   ?then=park  it has the first handle's number; a call on it from this request
//               does not wait for the request that added the first one, which is
//               still in the destructor
//   ?then=add   it has another number and is added to a multi handle of its own;
//               a call on it from this request waits until it is taken out, the
//               freed multi handle having gone in between
$then = (string) ($_GET['then'] ?? 'park');
unset($sharedState['curl_ch'], $sharedState['curl_mh'], $sharedState['curl_parked']);
$holder = curl_hold_start('mode=reuse&then=' . $then);

$ready = curl_hold_parked($sharedState) && isset($sharedState['curl_ch']);
$t->assertTrue('the first request built its handle and is waiting', $ready);
if (!$ready) {
    $t->done();
}

$ch = $sharedState['curl_ch'];
$threw = null;
$result = null;
$started = microtime(true);
try {
    $result = curl_setopt($ch, CURLOPT_USERAGENT, 'second');
} catch (\Throwable $e) {
    $threw = get_class($e) . ': ' . $e->getMessage();
}
$returned = microtime(true);
$held = oxphp_async_await($holder, 12.0);

$t->assertSame('curl_setopt() succeeded', $threw ?? $result, true);
if ($then === 'park') {
    $t->assertSame('the built handle has the freed handle\'s number',
        $held['built_id'] ?? $held, $held['first_id'] ?? null);
    $t->assertTrue(sprintf('curl_setopt() returned at once (%.3f s)', $returned - $started),
        $returned - $started < 0.2);
    $t->assertTrue('while the first request was still in the destructor',
        $returned < ($held['left_at'] ?? 0.0));
} else {
    $t->assertSame('an object took the freed handle\'s number',
        $held['placeholder_id'] ?? $held, $held['first_id'] ?? null);
    $t->assertTrue('so the built handle has another',
        ($held['built_id'] ?? null) !== ($held['first_id'] ?? null));
    $t->assertSame('the first request added it', $held['added'] ?? null, CURLM_OK);
    $t->assertSame('and took it out', $held['removed'] ?? null, CURLM_OK);
    $removedAt = $held['removed_at'] ?? INF;
    $t->assertTrue(
        sprintf('curl_setopt() returned only once the handle was out (%.3f s after it)', $returned - $removedAt),
        $returned >= $removedAt
    );
    $t->assertTrue('and soon after it, not at the end of a bound', $returned - $removedAt < 0.5);
}

unset($sharedState['curl_ch'], $sharedState['curl_mh'], $sharedState['curl_parked']);

$t->done();
