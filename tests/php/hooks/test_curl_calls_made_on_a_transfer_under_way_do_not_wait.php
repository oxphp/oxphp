<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';

$t = new TestCase('curl_calls_made_on_a_transfer_under_way_do_not_wait', 'hooks');

// A call on an easy handle that another request added to a multi handle waits for
// that request to take it out — except the calls made on a transfer under way,
// which HTTP clients make on other requests' transfers in a multi handle they
// share: curl_pause(), and curl_setopt() of CURLOPT_PRIVATE or CURLOPT_VERBOSE.
// Those return at once while the first request still has the handle in; any other
// curl_setopt() made after them still waits for it. A call among them that did wait
// would run until the first request gave up and took the handle out, so it fails
// the test, but leaves the calls after it nothing to wait for.
unset($sharedState['curl_release'], $sharedState['curl_cb_took']);
$holder = curl_hold_start('mode=member&how=wait&delay=100');

$ready = curl_hold_parked($sharedState) && isset($sharedState['curl_ch']);
$t->assertTrue('the first request added its handle and is waiting', $ready);
if (!$ready) {
    $sharedState['curl_release'] = true;
    $t->done();
}

$ch = $sharedState['curl_ch'];
$calls = [
    'curl_pause(CURLPAUSE_CONT)' => static fn() => curl_pause($ch, CURLPAUSE_CONT),
    'curl_setopt(CURLOPT_PRIVATE)' => static fn() => curl_setopt($ch, CURLOPT_PRIVATE, 'second'),
    'curl_setopt(CURLOPT_VERBOSE)' => static fn() => curl_setopt($ch, CURLOPT_VERBOSE, false),
];
$results = [];
foreach ($calls as $name => $call) {
    $started = microtime(true);
    try {
        $results[$name] = [$call(), microtime(true) - $started];
    } catch (\Throwable $e) {
        $results[$name] = [get_class($e) . ': ' . $e->getMessage(), microtime(true) - $started];
    }
}
$holderWaiting = (bool) ($sharedState['curl_parked'] ?? false);

// Released only now, so the first request takes the handle out after the call
// below has started waiting for it.
$sharedState['curl_release'] = true;
$other = null;
try {
    $other = curl_setopt($ch, CURLOPT_USERAGENT, 'second');
} catch (\Throwable $e) {
    $other = get_class($e) . ': ' . $e->getMessage();
}
$otherReturned = microtime(true);
$held = oxphp_async_await($holder, 12.0);

$t->assertTrue('the first request still had the handle in while they ran', $holderWaiting);
// The transfer has no connection yet, which libcurl answers with an error code;
// that it answered at all is what this checks.
$t->assertTrue('curl_pause() reached libcurl and returned its code', is_int($results['curl_pause(CURLPAUSE_CONT)'][0]));
$t->assertSame('curl_setopt(CURLOPT_PRIVATE) succeeded', $results['curl_setopt(CURLOPT_PRIVATE)'][0], true);
$t->assertSame('curl_setopt(CURLOPT_VERBOSE) succeeded', $results['curl_setopt(CURLOPT_VERBOSE)'][0], true);
foreach ($results as $name => [, $took]) {
    $t->assertTrue(sprintf('%s returned at once (%.3f s)', $name, $took), $took < 0.2);
}
$t->assertSame('curl_getinfo() reads the CURLOPT_PRIVATE it set', curl_getinfo($ch, CURLINFO_PRIVATE), 'second');
$t->assertSame('curl_setopt(CURLOPT_USERAGENT) succeeded', $other, true);
$t->assertSame("the first request's own curl_setopt() on it succeeded", $held['own_setopt'] ?? $held, true);
$t->assertTrue(sprintf('and did not wait for its own hold on the handle (%.3f s)', (float) ($held['own_setopt_took'] ?? INF)),
    ($held['own_setopt_took'] ?? INF) < 0.2);
$t->assertSame('the first request then took its handle out', $held['removed'] ?? $held, CURLM_OK);
$t->assertTrue(
    sprintf('and curl_setopt(CURLOPT_USERAGENT) returned only after that (%.3f s after it)',
        $otherReturned - ($held['removed_at'] ?? INF)),
    $otherReturned >= ($held['removed_at'] ?? INF)
);

unset($sharedState['curl_ch'], $sharedState['curl_mh'], $sharedState['curl_parked'],
    $sharedState['curl_release']);

$t->done();
