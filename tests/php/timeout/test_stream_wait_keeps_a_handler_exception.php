<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// Outside worker mode, a flush whose output handler threw waits for room on its
// worker thread, as any flush there does, and what the handler threw reaches the
// script's catch once the client reads.
//
// PHP hands the stream what the failed handler was given, and flushes it with
// the exception still pending. Nothing switches away from the request while it
// waits there, so the exception survives the wait, and the output goes out
// ahead of it. Every chunk of this stream goes through a handler that throws,
// so the flush that finds the queue full is one of those.

$t = new TestCase('stream_wait_keeps_a_handler_exception', 'timeout');

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
fwrite($sock, "GET /tests/timeout/fixture_stream_wait.php?limit=10&throw=1&token=$token HTTP/1.0\r\n"
    . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

// Inside the flush of a chunk whose handler threw, and still in that one 300 ms
// later: the exception is pending and the flush is waiting for room.
$waiting = $wait(static function (array $s) use ($read): bool {
    if ($s['stage'] !== 'thrown' || $s['ended']) {
        return false;
    }
    usleep(300_000);
    $again = $read();

    return $again !== null && $again === $s;
}, 5.0);
$t->assertNotNull('a flush whose handler threw waits for room', $waiting);
$t->assertSame(
    'with its exception not caught yet',
    ($waiting['thrown'] ?? 0) - ($waiting['caught'] ?? 0),
    1,
);

stream_set_timeout($sock, 10);
$received = 0;
while (!feof($sock)) {
    $piece = fread($sock, 262144);
    if ($piece === false || ($piece === '' && stream_get_meta_data($sock)['timed_out'])) {
        break;
    }
    $received += strlen($piece);
}
fclose($sock);

$ended = $wait(static fn (array $s): bool => $s['ended'], 5.0);
if (is_file($file)) {
    unlink($file);
}

$t->assertNotNull('the stream ended', $ended);
$t->assertSame('every chunk went through a handler that threw', $ended['thrown'] ?? null, 1024);
$t->assertSame('and every exception reached the catch around its flush', $ended['caught'] ?? null, 1024);
$t->assertNull('with no error', $ended['error'] ?? null);
$t->assertGreaterThan('the client got every chunk', $received, 1024 * 64 * 1024 - 1);

$t->done();
