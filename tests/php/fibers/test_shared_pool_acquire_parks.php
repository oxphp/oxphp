<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

use OxPHP\Shared\Pool;
use OxPHP\Shared\Registry;

// Shared\Pool::acquire() on a saturated pool waits for a slot to come back. The
// slot's holder can be another request parked on the same worker thread, or
// another task on the same async thread, and it gives the slot back only once it
// runs again — so the wait has to let it run. A wait that held the thread would
// leave the holder parked until the wait gave up, or for good.
//
// This profile runs one worker and one async thread, so the holder and the
// waiter always share a thread. The holder watches the pool's waiting() count in
// short parking sleeps, each timed: it can see the waiter waiting only if the
// waiter let it run, and a sleep that took far longer than asked for is the
// waiter holding the thread.

$t = new TestCase('shared_pool_acquire_parks', 'fibers');

$run = bin2hex(random_bytes(4));

$poolName = static fn (string $case): string => "pool-acquire-parks-$case-$run";
$pool = static fn (string $case): Pool => Registry::pool(
    $poolName($case),
    static fn (): Pool => new Pool(factory: static fn (): \stdClass => new \stdClass(), maxSize: 1)
);

/**
 * Watch `$p` until someone waits on it, for at most 2 s. Returns what waiting()
 * said last and how long the longest 10 ms sleep took.
 */
$watch = static function (Pool $p): array {
    $seen = 0;
    $longest = 0.0;
    $until = hrtime(true) + 2_000_000_000;
    while ($seen === 0 && hrtime(true) < $until) {
        $t0 = hrtime(true);
        oxphp_usleep(10_000);
        $longest = max($longest, (hrtime(true) - $t0) / 1e9);
        $seen = $p->stats()->waiting();
    }

    return [$seen, $longest];
};

/** Send one request to this server; the response is read later. */
$send = static function (string $query) {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return null;
    }
    stream_set_timeout($sock, 10);
    fwrite($sock, "GET /tests/fibers/fixture_shared_pool_acquire_parks.php?$query HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

    return $sock;
};

/** Read a response `$send` asked for, and decode its JSON body. */
$answer = static function ($sock): array {
    if ($sock === null) {
        return ['result' => 'connect failed'];
    }
    // Hooked (this profile hooks streams): the read parks this request's fiber.
    $resp = (string) stream_get_contents($sock);
    fclose($sock);
    $body = substr($resp, (int) strpos($resp, "\r\n\r\n") + 4);

    return json_decode($body, true) ?? ['result' => 'unparsed', 'response' => $resp];
};

/**
 * This request holds the one slot of a fresh pool while the fixture, on the
 * same worker thread, waits for it in `$mode`.
 */
$holdWhileRequestWaits = static function (string $mode) use ($t, $pool, $poolName, $watch, $send, $answer): bool {
    $p = $pool($mode);
    $h = $p->tryAcquire();
    $t->assertNotNull("$mode: this request holds the pool's one slot", $h);
    if ($h === null) {
        return false;
    }
    $h->get()->name = "holder-$mode";

    $sock = $send("mode=$mode&pool=" . urlencode($poolName($mode)));
    [$seen, $longest] = $watch($p);
    $h->release();
    $got = $answer($sock);

    $t->assertSame("$mode: the holder ran while the waiter waited, and saw it waiting", $seen, 1);
    $t->assertLessThan("$mode: the waiter did not hold the thread the holder needed", $longest, 1.0);
    $t->assertSame("$mode: the waiter got a slot", $got['result'] ?? null, 'acquired');
    $t->assertSame("$mode: the slot the holder gave back", $got['name'] ?? null, "holder-$mode");
    $t->assertSame("$mode: nobody is left waiting", $p->stats()->waiting(), 0);
    $t->meta("{$mode}_longest_sleep_s", round($longest, 3));
    $t->meta("{$mode}_waiter", $got);

    return $seen === 1 && ($got['result'] ?? null) === 'acquired';
};

// ── A request waits, a request holds ─────────────────────────────────────────

$boundedParks = $holdWhileRequestWaits('timeout');
$holdWhileRequestWaits('with');

// A wait with no bound that held the thread would hold this profile's only
// worker for good, and every test after this one with it. Run it only once a
// bounded wait has been seen to park.
if ($boundedParks) {
    $holdWhileRequestWaits('forever');
} else {
    $t->meta('forever', 'skipped: a bounded wait held the thread');
}

// ── A task waits, a task holds ───────────────────────────────────────────────

$p = $pool('task');
$holder = oxphp_async(static function () use ($p): array {
    $h = $p->acquire();
    $h->get()->name = 'holder-task';

    // $watch, inlined: a task gets its captured values copied to its own thread,
    // and a Pool travels that way where a closure does not.
    $seen = 0;
    $longest = 0.0;
    $until = hrtime(true) + 2_000_000_000;
    while ($seen === 0 && hrtime(true) < $until) {
        $t0 = hrtime(true);
        oxphp_usleep(10_000);
        $longest = max($longest, (hrtime(true) - $t0) / 1e9);
        $seen = $p->stats()->waiting();
    }
    $h->release();

    return ['seen' => $seen, 'longest' => $longest];
});
$waiter = oxphp_async(static function () use ($p): array {
    // Let the holder take the slot first.
    oxphp_usleep(50_000);
    try {
        $h = $p->acquireTimeout(3000);
        $got = $h->get()->name ?? null;
        $h->release();

        return ['result' => 'acquired', 'name' => $got];
    } catch (\Throwable $e) {
        return ['result' => get_class($e), 'message' => $e->getMessage()];
    }
});
$rh = oxphp_async_await($holder, 10.0);
$rw = oxphp_async_await($waiter, 10.0);

$t->assertSame('task: the holder ran while the waiter waited, and saw it waiting', $rh['seen'] ?? null, 1);
$t->assertLessThan('task: the waiter did not hold the thread the holder needed', $rh['longest'] ?? 99.0, 1.0);
$t->assertSame('task: the waiter got a slot', $rw['result'] ?? null, 'acquired');
$t->assertSame('task: the slot the holder gave back', $rw['name'] ?? null, 'holder-task');
$t->meta('task_longest_sleep_s', round((float) ($rh['longest'] ?? 0), 3));
$t->meta('task_waiter', $rw);

$t->done();
