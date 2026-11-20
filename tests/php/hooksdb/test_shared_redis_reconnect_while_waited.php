<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('shared_redis_reconnect_while_waited', 'hooksdb');

// One persistent Redis object shared between requests, with pooling off, so its
// connection is a stream phpredis registers under an id. One request holds it and
// loses it: its read times out, phpredis closes the stream, and the request's next
// command reconnects under the same id to a new one and parks there. Another
// request was waiting for the first stream all along. That stream being closed
// frees it, but the connection it was waiting for is now the new one, which the
// holder is parked on, so it has to go on waiting for the holder.
$host = getenv('DB_REDIS_HOST') ?: 'hooksdb-redis';

$fetch = static function (string $path): string {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return "connect failed: {$errstr} ({$errno})";
    }
    stream_set_timeout($sock, 12);
    fwrite($sock, "GET {$path} HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $response = (string) stream_get_contents($sock);
    fclose($sock);

    $split = strpos($response, "\r\n\r\n");

    return $split === false ? $response : substr($response, $split + 4);
};
$decode = static function (string $body, string $prefix): ?array {
    if (!str_starts_with($body, $prefix)) {
        return null;
    }
    $decoded = json_decode(substr($body, strlen($prefix)), true);

    return is_array($decoded) ? $decoded : null;
};

foreach (['object', 'hold_started', 'hold_done', 'retry_started', 'wait_started', 'wait_done',
          'wait_pending_at_loss'] as $edge) {
    unset($sharedState["reconnect_{$edge}"]);
}
$seed = new Redis();
$seed->connect($host, 6379, 3.0);
$seed->set('hooksdb:reconnect:probe', 'reconnect-probe-value');
$seed->del('hooksdb:reconnect:first', 'hooksdb:reconnect:second');
$seed->close();

$fixture = '/tests/hooksdb/fixture_redis_reconnect_while_waited.php?role=';
$t->assertSame('the object was built', oxphp_async_await(oxphp_async($fetch, $fixture . 'open')),
    'open-done');

$hold = oxphp_async($fetch, $fixture . 'hold');
$deadline = microtime(true) + 3.0;
while (!isset($sharedState['reconnect_hold_started']) && microtime(true) < $deadline) {
    oxphp_usleep(50_000);
}
// Well inside the holder's one-second read timeout.
oxphp_usleep(300_000);
$wait = oxphp_async($fetch, $fixture . 'wait');

$deadline = microtime(true) + 4.0;
while (!isset($sharedState['reconnect_retry_started']) && microtime(true) < $deadline) {
    oxphp_usleep(20_000);
}
// The waiter polls for the claim every few milliseconds; this is long enough for
// it to have gone ahead on the new connection if it is going to.
oxphp_usleep(300_000);

// Read here, before the release: once the holder is let go, either flag set
// afterwards says nothing about this window.
$retryParked = isset($sharedState['reconnect_retry_started']) && !isset($sharedState['reconnect_hold_done']);
$waiterStillWaiting = isset($sharedState['reconnect_wait_started']) && !isset($sharedState['reconnect_wait_done']);

$release = new Redis();
$release->connect($host, 6379, 3.0);
$release->rPush('hooksdb:reconnect:second', 'released');
$release->close();

$holdBody = oxphp_async_await($hold);
$waitBody = oxphp_async_await($wait);
$held = $decode($holdBody, 'hold-done:');
$waited = $decode($waitBody, 'wait-done:');
$t->meta('hold_body', $holdBody);
$t->meta('wait_body', $waitBody);

$t->assertTrue("the holder's first read gave up and lost the connection",
    str_starts_with((string) ($held['lost'] ?? ''), 'RedisException'));
$t->assertSame('the connection is persistent', $held['persistent_id'] ?? $holdBody, 'hooksdb-reconnect');
$t->assertTrue('the waiter was waiting when the holder lost the connection',
    $sharedState['reconnect_wait_pending_at_loss'] ?? false);
$t->assertTrue("the holder's second pop was parked on the new connection while the waiter ran", $retryParked);

$t->assertSame('the holder got the answer to its second pop', $held['popped'] ?? $holdBody,
    ['hooksdb:reconnect:second', 'released']);
$t->assertSame('the waiter got its own key', $waited['value'] ?? $waitBody, 'reconnect-probe-value');
$t->assertTrue('the waiter was still waiting at the release', $waiterStillWaiting);

$t->done();
