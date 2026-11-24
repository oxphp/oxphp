<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';

$t = new TestCase('curl_multi_exec_from_a_userland_fiber_is_refused_while_a_fiber_is_in_one', 'hooks');

// A loop of curl_multi_exec() and usleep() inside a userland Fiber, on a multi
// handle another request is in curl_multi_select() on. Neither call can suspend
// there, so the holder cannot go on while the loop runs: returned from at once,
// curl_multi_exec() would report the holder's transfer running for as long as the
// loop went on, cut off here after 200 calls. It is refused at its first call, as
// any call that would have to wait for the holder is in a userland Fiber.
unset($sharedState['curl_in_select']);
$holder = curl_hold_start('mode=multi&delay=2000');

$parked = curl_hold_parked($sharedState) && isset($sharedState['curl_mh']);
$deadline = microtime(true) + 3.0;
while ($parked && !($sharedState['curl_in_select'] ?? false) && microtime(true) < $deadline) {
    oxphp_usleep(20_000);
}
$inSelect = $parked && (bool) ($sharedState['curl_in_select'] ?? false);
$t->assertTrue('the holding request is in curl_multi_select() on the multi handle', $inSelect);
if (!$inSelect) {
    oxphp_async_await($holder, 12.0);
    $t->done();
}

$mh = $sharedState['curl_mh'];
$calls = 0;
$fiber = new Fiber(static function () use ($mh, &$calls): string {
    do {
        $calls++;
        $status = curl_multi_exec($mh, $running);
        usleep(1000);
    } while ($running > 0 && $status === CURLM_OK && $calls < 200);
    return 'returned ' . var_export($status, true);
});

$inSelectAtCall = (bool) ($sharedState['curl_in_select'] ?? false);
try {
    $fiber->start();
    $outcome = $fiber->isTerminated() ? (string) $fiber->getReturn() : 'suspended';
} catch (\OxPHP\Async\AsyncException $e) {
    $outcome = 'AsyncException: ' . $e->getMessage();
}

$t->meta('outcome', $outcome);
$t->meta('calls', $calls);
$t->assertTrue('the loop started while the holder was in curl_multi_select()', $inSelectAtCall);
$t->assertContains('curl_multi_exec() is refused', $outcome, 'AsyncException');
$t->assertContains('as a call that cannot suspend', $outcome, 'cannot be done here');
$t->assertSame('at the first call of the loop', $calls, 1);

$held = oxphp_async_await($holder, 12.0);
$t->assertSame("the holder's transfer got its whole answer", $held['body'] ?? $held, 'curl-done:/2000/holder');

unset($sharedState['curl_ch'], $sharedState['curl_mh'], $sharedState['curl_parked'], $sharedState['curl_in_select']);

$t->done();
