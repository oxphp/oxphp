<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';

$t = new TestCase('curl_easy_handle_out_of_its_multi_is_not_held', 'hooks');

// The request that added an easy handle to a multi handle keeps it until the
// handle is out again, and a call on it from elsewhere waits for that. Each way out
// ends the wait at once, while that request is still running — except the last,
// which is that request ending:
//
//   ?how=remove  this request takes the handle out with curl_multi_remove_handle(),
//                which does not itself wait for the request that added it
//   ?how=close   this request closes the multi handle
//   ?how=drop    the first request frees the multi handle without taking the
//                handle out
//   ?how=end     the first request ends with the handle still in
$how = (string) ($_GET['how'] ?? 'remove');
unset($sharedState['curl_release'], $sharedState['curl_cb_took']);
$holder = curl_hold_start('mode=member&delay=100&how=' . ($how === 'remove' || $how === 'close' ? 'wait' : $how));

if ($how === 'end') {
    $held = oxphp_async_await($holder, 12.0);
    $ready = isset($sharedState['curl_ch'], $sharedState['curl_mh']) && ($held['added'] ?? null) === CURLM_OK;
    $t->assertTrue('the first request added its handle and ended', $ready);
} else {
    $ready = curl_hold_parked($sharedState) && isset($sharedState['curl_ch']);
    $t->assertTrue('the first request added its handle and is waiting', $ready);
}
if (!$ready) {
    $sharedState['curl_release'] = true;
    $t->done();
}

$ch = $sharedState['curl_ch'];
$mh = $sharedState['curl_mh'] ?? null;
$threw = null;
$result = null;
$started = microtime(true);
try {
    if ($how === 'remove') {
        $result = curl_multi_remove_handle($mh, $ch);
        $t->assertTrue(sprintf('curl_multi_remove_handle() returned at once (%.3f s)', microtime(true) - $started),
            microtime(true) - $started < 0.2);
        $t->assertSame('and succeeded', $result, CURLM_OK);
    } elseif ($how === 'close') {
        curl_multi_close($mh);
    }
    $started = microtime(true);
    $result = curl_setopt($ch, CURLOPT_USERAGENT, 'second');
} catch (\Throwable $e) {
    $threw = get_class($e) . ': ' . $e->getMessage();
}
$took = microtime(true) - $started;
$holderWaiting = (bool) ($sharedState['curl_parked'] ?? false);

$sharedState['curl_release'] = true;
if ($how !== 'end') {
    $held = oxphp_async_await($holder, 12.0);
}

$t->assertSame('curl_setopt() succeeded', $threw ?? $result, true);
$t->assertTrue(sprintf('and returned at once (%.3f s)', $took), $took < 0.2);
if ($how !== 'end') {
    $t->assertTrue('while the first request was still waiting', $holderWaiting);
    $t->assertSame('which the release, not its own deadline, ended', $held['released'] ?? $held, true);
} else {
    curl_multi_remove_handle($mh, $ch);
}

unset($sharedState['curl_ch'], $sharedState['curl_mh'], $sharedState['curl_parked'],
    $sharedState['curl_release']);

$t->done();
