<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('shared_redis_reconnect_past_the_bound', 'hooksdb');

// One persistent Redis object shared between requests, with pooling off. One
// request holds it and keeps replacing its connection: each blocking read times
// out, phpredis closes the stream, and the next pop reconnects to a new one under
// the same id. Another request waits through all of that. Each closed stream frees
// it to go on waiting for the next, but its call still has the one bound every
// wait for a connection has — two seconds in this image — rather than a fresh
// bound for every connection the holder moves to.
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

foreach (['object', 'cycle_started', 'cycle_done', 'wait_started', 'wait_done', 'losses_while_waited'] as $edge) {
    unset($sharedState["bound_{$edge}"]);
}
$seed = new Redis();
$seed->connect($host, 6379, 3.0);
$seed->set('hooksdb:bound:probe', 'bound-probe-value');
$seed->del('hooksdb:bound:empty');
$seed->close();

$fixture = '/tests/hooksdb/fixture_redis_reconnect_past_the_bound.php?role=';
$t->assertSame('the object was built', oxphp_async_await(oxphp_async($fetch, $fixture . 'open')),
    'open-done');

$cycle = oxphp_async($fetch, $fixture . 'cycle');
$deadline = microtime(true) + 3.0;
while (!isset($sharedState['bound_cycle_started']) && microtime(true) < $deadline) {
    oxphp_usleep(50_000);
}
// Well inside the holder's first read timeout.
oxphp_usleep(200_000);
$wait = oxphp_async($fetch, $fixture . 'wait');

$cycleBody = oxphp_async_await($cycle);
$waitBody = oxphp_async_await($wait);
$cycled = $decode($cycleBody, 'cycle-done:');
$waited = $decode($waitBody, 'wait-done:');
$t->meta('cycle_body', $cycleBody);
$t->meta('wait_body', $waitBody);

$t->assertSame('the holder lost the connection on each of its four reads', $cycled['losses'] ?? $cycleBody,
    array_fill(0, 4, 'RedisException'));
// The premise: the connection was replaced under the waiter more than once, so a
// bound that started over with each one would not have run out by two seconds.
$t->assertGreaterThan('and lost it more than once while the waiter waited',
    $sharedState['bound_losses_while_waited'] ?? 0, 1);
$t->assertTrue('the waiter\'s call ended while the holder was still replacing the connection',
    $waited['cycling'] ?? false);

$t->assertTrue('the waiter gave up on the holder instead of sending its command: ' . ($waited['outcome'] ?? $waitBody),
    str_starts_with((string) ($waited['outcome'] ?? ''), 'RedisException: oxphp: another fiber holds this Redis connection'));
$seconds = (float) ($waited['seconds'] ?? 0.0);
$t->assertGreaterThan('after waiting out most of the bound', $seconds, 1.5);
$t->assertLessThan('and not past it', $seconds, 2.6);

$t->done();
