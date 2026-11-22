<?php

declare(strict_types=1);

// Channel::recv() and send(), which wait without a timeout, throw where a
// worker-mode request cannot park instead of waiting on the worker thread: the
// other end could be a request parked on the same worker, which cannot run
// until the call returns, and the wait would never end. They throw only when
// they would have to wait — a value already in the channel is received, a free
// slot taken, a closed channel reported — and they throw at once. That holds
// on a context nested in the request's as well, a userland Fiber or the
// engine's GC destructor fiber, which hold the same thread.
//
// The other end here is an oxphp_async() task, which answers 500 ms after this
// request hands the channel over, so a call that waits on the thread returns
// with the answer rather than hanging the worker. It is this request's task
// rather than the inner request's, since the end of a request cancels the
// tasks it did not await, and one of the waits runs after the inner request
// has ended.

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/thread_cell_probe.php';

// A closure rather than a function: the worker keeps a declared function for as
// long as it lives, and running this file a second time would declare it again.
$waitFor = static function (array &$state, string $key, float $seconds = 5.0): void {
    $deadline = microtime(true) + $seconds;
    while (!isset($state[$key]) && microtime(true) < $deadline) {
        oxphp_usleep(10_000);
    }
};

$fixture = '/tests/fibers/fixture_wait_without_a_timeout_where_a_request_cannot_park.php';
$cases = ['filter-recv', 'multisort-recv', 'multisort-fiber-recv', 'multisort-gc-recv', 'superglobal-recv', 'filter-send', 'filter-recv-ready', 'filter-send-room', 'filter-recv-closed'];
$results = [];
$partners = [];
foreach ($cases as $case) {
    unset($sharedState["forever_$case"]);
    $channel = new OxPHP\Shared\Channel(1);
    $partner = null;
    if ($case === 'filter-send') {
        $channel->send('held');
        $partner = oxphp_async(static function () use ($channel): string {
            oxphp_usleep(500_000);

            return $channel->recvTimeout(2000)->status()->name;
        });
    } elseif ($case === 'filter-recv-ready') {
        $channel->send('ready');
    } elseif ($case === 'filter-recv-closed') {
        $channel->close();
    } elseif ($case !== 'filter-send-room') {
        $partner = oxphp_async(static function () use ($channel): string {
            oxphp_usleep(500_000);

            return $channel->sendTimeout('late', 2000)->status()->name;
        });
    }
    $sharedState['forever_channel'] = $channel;

    thread_cell_read(thread_cell_send("$fixture?case=$case&k=1"));
    // The superglobal case reports after its response has gone out.
    $waitFor($sharedState, "forever_$case");
    $results[$case] = $sharedState["forever_$case"] ?? null;
    if ($partner !== null) {
        $partners[$case] = oxphp_async_await($partner, 5.0);
    }
    unset($sharedState['forever_channel']);
}

$t = new TestCase('a_channel_wait_without_a_timeout_throws_where_a_request_cannot_park', 'fibers');
$t->meta('results', $results);
$t->meta('partners', $partners);

$recvRefused = 'OxPHP\Shared\SharedException: Channel::recv() found nothing to receive and cannot wait without a timeout where the code running cannot be suspended; use recvTimeout()';
$sendRefused = 'OxPHP\Shared\SharedException: Channel::send() found the channel full and cannot wait without a timeout where the code running cannot be suspended; use sendTimeout()';
foreach (['filter-recv', 'multisort-recv', 'multisort-fiber-recv', 'multisort-gc-recv', 'superglobal-recv'] as $case) {
    $t->assertSame("$case: recv() throws", $results[$case]['outcome'] ?? null, $recvRefused);
    $t->assertTrue("$case: recv() threw without waiting", ($results[$case]['elapsed'] ?? 1.0) < 0.1);
}
$t->assertTrue('multisort-gc-recv: the destructor ran on a fiber nested in the request\'s', ($results['multisort-gc-recv']['nested'] ?? false) === true);
$t->assertTrue('multisort-gc-recv: the destructor ran inside gc_collect_cycles(), in the sort', ($results['multisort-gc-recv']['collecting'] ?? false) === true);
$t->assertSame('filter-send: send() throws', $results['filter-send']['outcome'] ?? null, $sendRefused);
$t->assertTrue('filter-send: send() threw without waiting', ($results['filter-send']['elapsed'] ?? 1.0) < 0.1);
$t->assertSame('filter-recv-ready: recv() takes the value already there', $results['filter-recv-ready']['outcome'] ?? null, 'value: ready');
$t->assertSame('filter-send-room: send() takes the free slot', $results['filter-send-room']['outcome'] ?? null, 'status: Ok');
$t->assertSame('filter-recv-closed: recv() reports the closed channel', $results['filter-recv-closed']['outcome'] ?? null, 'status: Closed');

$t->done();
