<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

use OxPHP\Server\Worker;
use OxPHP\Shared\Counter;
use OxPHP\Shared\Pool;
use OxPHP\Shared\Registry;

// A Pool slot left idle for longer than idleTimeoutMs is destroyed by the
// worker that owns it. In worker mode that worker is usually waiting for its
// next request by then, so the eviction has to reach it there: the next request
// it serves would otherwise take the stale slot first and make it fresh again.
//
// This profile runs two workers and no runtime hooks. This request blocks the
// worker it runs on for as long as it lasts, so the fixture lands on the other
// one, and nothing reaches that one again until the checks below are done —
// the eviction has to happen while it waits.

$t = new TestCase('shared_pool_idle_eviction', 'worker');

$run = bin2hex(random_bytes(4));
$me = Worker::current()->id();

/** One request to this server, read to its end. */
$get = static function (string $query): string {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return "connect failed: $errstr";
    }
    stream_set_timeout($sock, 10);
    fwrite($sock, "GET /tests/worker/fixture_shared_pool_idle.php?$query HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $resp = (string) stream_get_contents($sock);
    fclose($sock);

    return $resp;
};

$workerOf = static fn (string $resp): ?int
    => preg_match('/^OK worker=(\d+)$/m', $resp, $m) ? (int) $m[1] : null;

/** One sample of a series, or null when the scrape does not carry it. */
$metric = static function (string $series): ?float {
    $text = (string) @file_get_contents('http://127.0.0.1:9090/metrics');
    return preg_match('/^' . preg_quote($series, '/') . ' ([\d.e+-]+)$/m', $text, $m)
        ? (float) $m[1]
        : null;
};

/** Poll until $done() holds or $seconds pass. Blocks this worker, on purpose. */
$waitFor = static function (callable $done, float $seconds): bool {
    $deadline = microtime(true) + $seconds;
    while (!$done()) {
        if (microtime(true) > $deadline) {
            return false;
        }
        usleep(50_000);
    }
    return true;
};

// ── A stale slot on a waiting worker ─────────────────────────────────────────

$resp = $get("case=idle&run=$run");
$owner = $workerOf($resp);
$t->assertMatch('the fixture answered', $resp, '#^HTTP/\S+ 200 #');
$t->assertTrue('the fixture ran on the other worker', $owner !== null && $owner !== $me);

$calls = Registry::counter("pool-idle-calls-$run", static fn (): Counter => new Counter());
$p = Registry::pool(
    "pool-idle-$run",
    static fn (): Pool => new Pool(factory: static fn (): \stdClass => new \stdClass(), destroy: null, maxSize: 1)
);
$t->assertSame('the slot is idle before its timeout', $p->stats()->idle(), 1);

// Read once the fixture's request has been counted, and again after the
// eviction: equal, so the owner was woken for it rather than handed a request.
usleep(200_000);
$servedBefore = $metric("oxphp_worker_requests_count{worker=\"$owner\"}");

$waitFor(static fn (): bool => $calls->get() >= 1, 4.0);

$t->assertSame('$destroy ran once', $calls->get(), 1);
$t->assertSame('the pool holds no slot', $p->stats()->size(), 0);
$t->assertSame('none of them idle', $p->stats()->idle(), 0);
$t->assertSame(
    'the slot went by idle timeout, not by evict()',
    $metric('oxphp_shared_pool_evicted_total{pool_id="' . $p->id() . '",reason="idle_timeout"}'),
    1.0
);
$t->assertSame(
    'the owner served no request meanwhile',
    $metric("oxphp_worker_requests_count{worker=\"$owner\"}"),
    $servedBefore
);

// ── Output and shutdown functions from $destroy ──────────────────────────────
// Written or registered with no request in flight, so they belong to no
// response — least of all to that of the request the worker serves next,
// whether written straight out or into a buffer $destroy left open.

$resp = $get("case=echo&run=$run");
$owner = $workerOf($resp);
$t->assertTrue('the echo fixture ran on the other worker', $owner !== null && $owner !== $me);

$calls = Registry::counter("pool-idle-echo-calls-$run", static fn (): Counter => new Counter());
$waitFor(static fn (): bool => $calls->get() >= 1, 4.0);
$t->assertSame('the echoing $destroy ran', $calls->get(), 1);

$resp = $get("case=none&run=$run");
$t->assertSame('the next request ran on the same worker', $workerOf($resp), $owner);
$t->assertTrue('what $destroy wrote out did not reach its response', !str_contains($resp, "LEAK-$run-plain"));
$t->assertTrue('nor what it left in an open buffer', !str_contains($resp, "LEAK-$run-buffered"));
$t->assertTrue('and its shutdown function did not run there', !str_contains($resp, "LEAK-$run-shutdown"));

// Buffers $destroy leaves open are given up one by one, and what releasing one
// runs may open another: none of them may outlive its release as the active
// buffer, which the next call into output buffering would read from freed memory.
$resp = $get("case=released&run=$run");
$owner = $workerOf($resp);
$t->assertTrue('the released-buffers fixture ran on the other worker', $owner !== null && $owner !== $me);

$calls = Registry::counter("pool-idle-released-calls-$run", static fn (): Counter => new Counter());
$stale = Registry::counter("pool-idle-released-stale-$run", static fn (): Counter => new Counter());
$waitFor(static fn (): bool => $calls->get() >= 1, 4.0);
$t->assertSame('the $destroy leaving buffers open ran', $calls->get(), 1);

$resp = $get("case=none&run=$run");
$t->assertSame('the worker serves on after giving them up', $workerOf($resp), $owner);
$t->assertSame('no released buffer was still reported as active', $stale->get(), 0);
$t->assertSame(
    'the buffer a release opened was released with its callback',
    Registry::counter("pool-idle-released-started-$run", static fn (): Counter => new Counter())->get(),
    1
);

// What $destroy leaves behind runs PHP again as it is given up: a destructor
// that throws there, one that registers a shutdown function. Neither may
// outlive the pass that gave it up — an exception left standing would make the
// next $destroy on that worker return without running.
$resp = $get("case=late&run=$run");
$owner = $workerOf($resp);
$t->assertTrue('the late-cleanup fixture ran on the other worker', $owner !== null && $owner !== $me);

$calls = Registry::counter("pool-idle-late-calls-$run", static fn (): Counter => new Counter());
$freed = Registry::counter("pool-idle-late-freed-$run", static fn (): Counter => new Counter());
$nested = Registry::counter("pool-idle-late-nested-$run", static fn (): Counter => new Counter());
$second = Registry::pool(
    "pool-idle-late-second-$run",
    static fn (): Pool => new Pool(factory: static fn (): \stdClass => new \stdClass(), destroy: null, maxSize: 1)
);
$waitFor(static fn (): bool => $second->stats()->size() === 0, 5.0);
$t->assertSame('the next pass evicted the second slot', $second->stats()->size(), 0);
$t->assertSame('and ran $destroy on it', $calls->get(), 1);
$t->assertSame('what a released buffer registered was given up in the pass before', $freed->get(), 1);
$t->assertSame('and so was what a released shutdown function registered', $nested->get(), 1);

// ── What $destroy keeps on the heap ──────────────────────────────────────────
// A worker retires itself when a request ended inside an internal function grew
// its heap past a threshold since it last had no request in flight. The eviction
// runs at such a moment, so what $destroy keeps there is no request's growth:
// the next request ended that way, leaving little, keeps its worker.

$resp = $get("case=heap&run=$run");
$owner = $workerOf($resp);
$t->assertTrue('the heap fixture ran on the other worker', $owner !== null && $owner !== $me);

$calls = Registry::counter("pool-idle-heap-calls-$run", static fn (): Counter => new Counter());
usleep(200_000);
$servedBefore = $metric("oxphp_worker_requests_count{worker=\"$owner\"}");
$waitFor(static fn (): bool => $calls->get() >= 1, 4.0);
$t->assertSame('the $destroy keeping memory ran', $calls->get(), 1);
$t->assertSame(
    'with no request on its worker since',
    $metric("oxphp_worker_requests_count{worker=\"$owner\"}"),
    $servedBefore
);

$retiredBefore = $metric('oxphp_worker_recycles_by_reason_total{reason="scheduled"}') ?? 0.0;
$resp = $get("case=sort&run=$run");
$t->assertMatch('the next request was ended by its time limit', $resp, '#^HTTP/\S+ 504 #');
$t->assertSame(
    'on that worker',
    $metric("oxphp_worker_requests_count{worker=\"$owner\"}"),
    $servedBefore + 1.0
);
$waitFor(
    static fn (): bool => ($metric('oxphp_worker_recycles_by_reason_total{reason="scheduled"}') ?? 0.0) > $retiredBefore,
    1.5
);
$t->assertSame(
    'which was not retired for what $destroy kept',
    $metric('oxphp_worker_recycles_by_reason_total{reason="scheduled"}') ?? 0.0,
    $retiredBefore
);

// ── A fatal in $destroy during the eviction ──────────────────────────────────
// No request is left to end, so the worker is retired, as for a fatal raised by
// the cycle collector between requests.

$retiredBefore = $metric('oxphp_worker_recycles_by_reason_total{reason="scheduled"}') ?? 0.0;

$resp = $get("case=fatal&run=$run");
$owner = $workerOf($resp);
$t->assertMatch('the fatal fixture answered', $resp, '#^HTTP/\S+ 200 #');
$t->assertTrue('it ran on the other worker', $owner !== null && $owner !== $me);

$calls = Registry::counter("pool-idle-fatal-calls-$run", static fn (): Counter => new Counter());
$p = Registry::pool(
    "pool-idle-fatal-$run",
    static fn (): Pool => new Pool(factory: static fn (): \stdClass => new \stdClass(), destroy: null, maxSize: 2)
);

$waitFor(
    static fn (): bool => ($metric('oxphp_worker_recycles_by_reason_total{reason="scheduled"}') ?? 0.0) > $retiredBefore,
    4.0
);
// The worker's exit runs past the point the recycle is counted at.
$waitFor(static fn (): bool => $p->stats()->size() === 0, 1.0);

$t->assertSame(
    'the worker was retired',
    $metric('oxphp_worker_recycles_by_reason_total{reason="scheduled"}'),
    $retiredBefore + 1.0
);
// Nothing ran after the fatal: $destroy was not called on the newer slot.
$t->assertSame('$destroy ran once', $calls->get(), 1);
// The slot the fatal was destroying counts as gone, and the one it never
// reached went with the worker that held it.
$t->assertSame('the pool holds no slot', $p->stats()->size(), 0);

// Its replacement serves Shared calls.
for ($i = 1; $i <= 2; $i++) {
    $t->assertMatch("request $i after the retire completes", $get("case=none&run=$run"), '#^HTTP/\S+ 200 #');
}

$t->done();
