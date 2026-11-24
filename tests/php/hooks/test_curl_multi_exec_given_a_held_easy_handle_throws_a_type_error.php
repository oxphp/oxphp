<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';

$t = new TestCase('curl_multi_exec_given_a_held_easy_handle_throws_a_type_error', 'hooks');

// curl_multi_exec() on a multi handle another request is in a call on returns at
// once, without going into ext/curl. Given an easy handle another request is in
// curl_exec() on, it is not taken for such a multi handle: it waits for the holder
// like any other call on that handle, and ext/curl then refuses the argument with
// the TypeError PHP throws without the hook.
$holder = curl_hold_start('mode=easy&delay=1000');

$parked = curl_hold_parked($sharedState) && isset($sharedState['curl_ch']);
$t->assertTrue('the holding request reached its curl_exec() and is parked in it', $parked);
if (!$parked) {
    oxphp_async_await($holder, 12.0);
    $t->done();
}

$ch = $sharedState['curl_ch'];
$parkedAtCall = (bool) ($sharedState['curl_parked'] ?? false);
try {
    $outcome = 'returned ' . var_export(curl_multi_exec($ch, $running), true);
} catch (\TypeError $e) {
    $outcome = 'TypeError: ' . $e->getMessage();
}

$t->meta('outcome', $outcome);
$t->assertTrue('the call was made while the holder was in curl_exec()', $parkedAtCall);
$t->assertContains('ext/curl refused the easy handle with a TypeError', $outcome, 'TypeError');
$t->assertContains('naming the class it wants', $outcome, 'CurlMultiHandle');

$held = oxphp_async_await($holder, 12.0);
$t->assertSame("the holder's transfer got its whole answer", $held['body'] ?? $held, 'curl-done:/1000/holder');

unset($sharedState['curl_ch'], $sharedState['curl_parked']);

$t->done();
