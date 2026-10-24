<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

use OxPHP\Server\Worker;
use OxPHP\Shared\Counter;
use OxPHP\Shared\Registry;

// A $destroy that runs out of memory while the idle-timeout eviction runs it
// retires the worker, as a fatal there does: no request is left to end.
//
// From PHP 8.4.27 / 8.5.12 the engine reports running out of memory itself and
// swallows the bailout its report ends in, so no fatal reached the eviction, and
// the worker was kept with every object it holds flagged as destructed, as a
// fatal leaves them.
//
// Kept out of test_shared_pool_idle_eviction, which already runs close to the
// runner's time limit. This profile runs two workers and no runtime hooks. This
// request blocks the worker it runs on for as long as it lasts, so the fixture
// lands on the other one, and the eviction happens while that one waits.

$t = new TestCase('shared_pool_idle_eviction_oom', 'worker');

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

$retiredBefore = $metric('oxphp_worker_recycles_by_reason_total{reason="scheduled"}') ?? 0.0;

$resp = $get("case=oom&run=$run");
$owner = $workerOf($resp);
$t->assertMatch('the fixture answered', $resp, '#^HTTP/\S+ 200 #');
$t->assertTrue('it ran on the other worker', $owner !== null && $owner !== $me);

$calls = Registry::counter("pool-idle-oom-calls-$run", static fn (): Counter => new Counter());
$waitFor(
    static fn (): bool => ($metric('oxphp_worker_recycles_by_reason_total{reason="scheduled"}') ?? 0.0) > $retiredBefore,
    4.0
);

$t->assertSame('$destroy ran out of memory', $calls->get(), 1);
$t->assertSame(
    'and the worker was retired for it',
    $metric('oxphp_worker_recycles_by_reason_total{reason="scheduled"}'),
    $retiredBefore + 1.0
);

// Its replacement serves Shared calls.
for ($i = 1; $i <= 2; $i++) {
    $t->assertMatch("request $i after the retire completes", $get("case=none&run=$run"), '#^HTTP/\S+ 200 #');
}

$t->done();
