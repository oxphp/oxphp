<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';

$t = new TestCase('curl_guard_refuses_from_a_userland_fiber', 'hooks');

// A cURL call on a handle another request is parked in the middle of, made from
// inside a userland Fiber. Waiting for the holder means suspending, and a userland
// fiber's context is not one the server can suspend; running the call anyway is
// what the wait exists to prevent. So it is refused at once, with an exception the
// caller can catch, and the holder's transfer goes on undisturbed. The refused call
// sets a new URL, which the handle would report as its effective URL had it run.
$holder = curl_hold_start('mode=easy&delay=2000');

$parked = curl_hold_parked($sharedState) && isset($sharedState['curl_ch']);
$t->assertTrue('the holding request reached its curl_exec() and is parked in it', $parked);
if (!$parked) {
    $t->done();
}

$ch = $sharedState['curl_ch'];
$fiber = new Fiber(static function () use ($ch): string {
    curl_setopt($ch, CURLOPT_URL, 'http://' . net_peer_ip() . ':8080/0/intruder');
    return 'ran';
});

$started = microtime(true);
$outcome = 'nothing';
$message = '';
try {
    $fiber->start();
    $outcome = $fiber->isTerminated() ? 'ran' : 'suspended';
} catch (\OxPHP\Async\AsyncException $e) {
    $outcome = 'refused';
    $message = $e->getMessage();
}
$elapsed = microtime(true) - $started;

$t->assertSame('the call was refused with an AsyncException', $outcome, 'refused');
$t->assertContains('which names cURL as the reason', $message, 'cURL');
$t->assertLessThan('at once, not after waiting for the holder', $elapsed, 0.5);

// A getter is not held up by the holder at all: it changes nothing a parked
// transfer depends on.
$getter = new Fiber(static fn() => curl_getinfo($ch, CURLINFO_EFFECTIVE_URL));
$getter->start();
$t->assertContains(
    "curl_getinfo() on the same handle still answers, with the holder's URL",
    $getter->isTerminated() ? (string) $getter->getReturn() : 'getter suspended',
    '/2000/holder'
);

$held = oxphp_async_await($holder, 12.0);
$t->assertSame("the holder's transfer got its whole answer", $held['body'] ?? $held, 'curl-done:/2000/holder');

unset($sharedState['curl_ch'], $sharedState['curl_parked']);

$t->done();
