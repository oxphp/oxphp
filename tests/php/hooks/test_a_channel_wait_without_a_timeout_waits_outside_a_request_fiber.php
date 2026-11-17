<?php

declare(strict_types=1);

// Outside a fiber of the server's — a request in traditional mode —
// Channel::recv(), which has no timeout, waits on the thread for a value, even
// inside array_multisort(), where fiber switching is blocked. Only a request or
// task the server runs on a fiber shares its thread with others that could be
// the other end of the channel; this request does not, so it has no reason to
// refuse the wait.

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/../fibers/multisort_probe.php';

$recvAnsweredLater = static function (): array {
    $channel = new OxPHP\Shared\Channel(1);
    $partner = oxphp_async(static function () use ($channel): string {
        oxphp_usleep(100_000);

        return $channel->sendTimeout('late', 2000)->status()->name;
    });
    $start = microtime(true);
    try {
        $result = $channel->recv();
        $outcome = $result->isOk() ? 'value: ' . $result->value() : 'status: ' . $result->status()->name;
    } catch (\Throwable $e) {
        $outcome = get_class($e) . ': ' . $e->getMessage();
    }
    $elapsed = microtime(true) - $start;
    oxphp_async_await($partner, 5.0);

    return ['outcome' => $outcome, 'elapsed' => $elapsed];
};

$plain = $recvAnsweredLater();
$inSort = null;
multisort_probe_sort(static function () use ($recvAnsweredLater, &$inSort): void {
    $inSort = $recvAnsweredLater();
});

$t = new TestCase('a_channel_wait_without_a_timeout_waits_outside_a_request_fiber', 'hooks');
$t->meta('plain', $plain);
$t->meta('in_sort', $inSort);
$t->assertSame('recv() receives the value sent later', $plain['outcome'], 'value: late');
$t->assertTrue('recv() waited for it', $plain['elapsed'] >= 0.09);
$t->assertSame('inside array_multisort(), recv() receives the value sent later', $inSort['outcome'] ?? null, 'value: late');
$t->assertTrue('inside array_multisort(), recv() waited for it', ($inSort['elapsed'] ?? 0.0) >= 0.09);

$t->done();
