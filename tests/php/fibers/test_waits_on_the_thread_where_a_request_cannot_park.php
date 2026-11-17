<?php

declare(strict_types=1);

// A Shared\Channel wait with a timeout, oxphp_async_await_any() and
// oxphp_async_await_race() made where the request cannot park wait on the
// worker thread, as they do outside a fiber, instead of failing. Each asks the
// server whether the caller is on a fiber that can park, and takes a path that
// parks it if so; that path cannot park where switching or parking is blocked,
// so the answer has to be no there.
//
// Three places a request cannot park, each blocked in its own way: a
// FILTER_CALLBACK of filter_input_array() and a comparison of array_multisort()
// block fiber switching, and the release of a request's superglobals blocks
// the server's parking while a Fiber can still switch. Whether a Fiber can
// switch is read at the point of the wait, so each case shows which of the two
// it exercises, and whether the wait held the thread is read from this
// request: it keeps trying to run every 10 ms, and must get no turn inside a
// channel wait.

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/thread_cell_probe.php';

// Closures rather than functions: the worker keeps a declared function for as
// long as it lives, and running this file a second time would declare it again.

// Waits, parked, until $state[$key] is set, noting each turn it gets.
$waitFor = static function (array &$state, string $key, array &$turns, float $seconds = 5.0): void {
    $deadline = microtime(true) + $seconds;
    while (!isset($state[$key]) && microtime(true) < $deadline) {
        $turns[] = microtime(true);
        oxphp_usleep(10_000);
    }
};

$fixture = '/tests/fibers/fixture_wait_where_a_request_cannot_park.php';
$channelCases = ['filter-channel', 'multisort-channel', 'superglobal-channel'];
$waits = [];
$turns = [];
foreach ($channelCases as $case) {
    unset($sharedState["wait_$case"]);
    $turns[$case] = [];
    $sock = thread_cell_send("$fixture?case=$case&k=1");
    $waitFor($sharedState, "wait_$case", $turns[$case]);
    thread_cell_read($sock);
    $waits[$case] = $sharedState["wait_$case"] ?? null;
}
$awaits = [];
foreach (['filter-await-any', 'filter-await-race'] as $case) {
    $awaits[$case] = json_decode(thread_cell_read(thread_cell_send("$fixture?case=$case&k=1"))['body'], true);
}

$t = new TestCase('waits_on_the_thread_where_a_request_cannot_park', 'fibers');
$t->meta('waits', $waits);
$t->meta('awaits', $awaits);

$blocked = 'FiberError: Cannot switch fibers in current execution context';
$t->assertSame('a FILTER_CALLBACK cannot switch fibers', $waits['filter-channel']['switch'] ?? null, $blocked);
$t->assertSame('a comparison of array_multisort() cannot switch fibers', $waits['multisort-channel']['switch'] ?? null, $blocked);
$t->assertSame('a superglobal destructor can switch fibers', $waits['superglobal-channel']['switch'] ?? null, 'switched');

foreach ($channelCases as $case) {
    $t->assertSame("$case: recvTimeout() timed out", $waits[$case]['wait'] ?? null, 'Timeout');
    $t->assertTrue("$case: recvTimeout() waited its 100 ms", ($waits[$case]['elapsed'] ?? 0.0) >= 0.09);
    $window = $waits[$case]['window'] ?? null;
    $t->assertTrue(
        "$case: this request was trying to run before the wait began",
        $window !== null && $turns[$case] !== [] && $turns[$case][0] < $window[0]
    );
    $inside = $window === null ? [] : array_values(array_filter(
        $turns[$case],
        static fn (float $at): bool => $at > $window[0] && $at < $window[1]
    ));
    $t->assertSame("$case: no turn of this request fell inside the wait", $inside, []);
}

foreach ($awaits as $case => $await) {
    $t->assertTrue("$case: returned a task result", in_array($await['wait'] ?? null, [1, 2], true));
    $t->assertTrue("$case: waited for the tasks", ($await['elapsed'] ?? 0.0) >= 0.05);
}

$t->done();
