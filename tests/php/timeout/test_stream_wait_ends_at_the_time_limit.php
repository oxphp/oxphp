<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// Outside worker mode, a stream waiting for room in its queue is still ended by
// its time limit.
//
// The stream sets a two-second limit, and its client — this request, on the
// profile's other worker — connects and reads nothing, so the stream spends
// nearly all of that limit inside one flush, waiting on its worker thread. A
// wait that did not look at the engine's pending timeout would last for as long
// as the client stays connected. Both edges are read: the limit is what ended
// it, which it cannot have reached early, and it was ended close to the limit
// rather than when this request gave up and left.

$t = new TestCase('stream_wait_ends_at_the_time_limit', 'timeout');

$token = bin2hex(random_bytes(8));
$file = sys_get_temp_dir() . "/oxphp-stream-wait-$token.json";

// The fixture renames the file into place and never removes it.
$read = static function () use ($file): ?array {
    $raw = is_file($file) ? file_get_contents($file) : false;
    $state = is_string($raw) ? json_decode($raw, true) : null;

    return is_array($state) ? $state : null;
};

// The stream's state once $until accepts it, polled for at most $seconds.
$wait = static function (callable $until, float $seconds) use ($read): ?array {
    $deadline = microtime(true) + $seconds;
    do {
        $state = $read();
        if ($state !== null && $until($state)) {
            return $state;
        }
        usleep(10_000);
    } while (microtime(true) < $deadline);

    return null;
};

$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue('connected to the stream', $sock !== false);
fwrite($sock, "GET /tests/timeout/fixture_stream_wait.php?limit=2&token=$token HTTP/1.0\r\n"
    . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

// Inside a flush, and still in that one 200 ms later.
$stalled = $wait(static function (array $s) use ($read): bool {
    if ($s['stage'] !== 'flushing' || $s['ended']) {
        return false;
    }
    usleep(200_000);
    $again = $read();

    return $again !== null && $again === $s;
}, 5.0);
$t->assertNotNull('the stream is waiting in a flush', $stalled);

$ended = $wait(static fn (array $s): bool => $s['ended'], 6.0);
fclose($sock);
if (is_file($file)) {
    unlink($file);
}

$ran = (float) ($ended['ran'] ?? 0.0);

$t->assertNotNull('the stream ended while its client was still connected', $ended);
$t->assertSame('by its time limit', ($ended['status'] ?? 0) & CONNECTION_TIMEOUT, CONNECTION_TIMEOUT);
$t->assertContains('as the limit reports itself', (string) ($ended['error'] ?? ''), 'Maximum execution time');
$t->assertSame('from inside the flush it was waiting in', $ended['stage'] ?? null, 'flushing');
$t->assertGreaterThan('not before the limit (ran ' . round($ran, 3) . ' s)', $ran, 1.95);
$t->assertLessThan('and not long after it', $ran, 3.0);

$t->done();
