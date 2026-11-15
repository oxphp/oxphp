<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';

$t = new TestCase('curl_easy_added_under_an_exception_is_held', 'hooks');

// curl_multi_add_handle() can put the handle in and still leave an exception
// behind: ext/curl warns about a CURLOPT_FILE stream that is gone, an error handler
// that throws for warnings — as frameworks install — makes that an exception, and
// the add goes ahead under it. The handle is then in the first request's multi
// handle like any other, and a second request's curl_setopt() on it waits until the
// first request takes it out.
unset($sharedState['curl_release']);
$holder = curl_hold_start('mode=member&how=wait&warned=1&delay=100');

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
$t->assertTrue(
    'its curl_multi_add_handle() threw the warning about CURLOPT_FILE',
    str_contains((string) ($held['add_threw'] ?? json_encode($held)), 'CURLOPT_FILE resource has gone away')
);
$t->assertSame('and put the handle in all the same', $held['added_again'] ?? $held, CURLM_ADDED_ALREADY);
$t->assertSame('curl_setopt() succeeded', $set, true);
$t->assertSame('the first request then took its handle out', $held['removed'] ?? $held, CURLM_OK);
$t->assertTrue(
    sprintf('and curl_setopt() returned only after that (%.3f s after it)',
        $returned - ($held['removed_at'] ?? INF)),
    $returned >= ($held['removed_at'] ?? INF)
);

unset($sharedState['curl_ch'], $sharedState['curl_mh'], $sharedState['curl_parked'],
    $sharedState['curl_release']);

$t->done();
