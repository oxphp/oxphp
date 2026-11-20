<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('shared_pconnect_moves_held_object', 'hooksdb');

// One Redis object shared between requests. One request is parked reading the
// reply to a blocking pop through it, and another calls pconnect() on it with
// another persistent id. phpredis gives the object a new socket on every connect
// and frees the old one, which the holder's read is still going through, so the
// call has to wait for the holder, the way any other call through the object does.
$host = getenv('DB_REDIS_HOST') ?: 'hooksdb-redis';

$fetch = static function (string $path): string {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return "connect failed: {$errstr} ({$errno})";
    }
    stream_set_timeout($sock, 10);
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

foreach (['object', 'hold_started', 'hold_done', 'move_started', 'move_done'] as $edge) {
    unset($sharedState["moves_{$edge}"]);
}
$seed = new Redis();
$seed->connect($host, 6379, 3.0);
$seed->del('hooksdb:moves:list');
$seed->close();

// How long the mover is left alone with the holder parked before the holder is
// released. A pconnect() that does not wait returns in milliseconds.
$holdFor = 0.5;

$fixture = '/tests/hooksdb/fixture_pconnect_moves_held_object.php?role=';
$openBody = oxphp_async_await(oxphp_async($fetch, $fixture . 'open'));
$opened = $decode($openBody, 'open-done:');
$t->meta('open_body', $openBody);
$t->assertNotNull('the object was opened', $opened);
// Whether the connection is persistent is phpredis's call: under ZTS 6.3.0 turns
// pconnect() into a plain connect, which getPersistentID() reports by answering
// null, and the develop commit the fixture installs on PHP 8.6 does not. Either
// way the move must wait; only a persistent one is keyed by something other than
// the object, which is what this test is for.
$persistent = ($opened['persistent_id'] ?? null) === 'hooksdb-moves-a';
$t->meta('persistent', $persistent);
if (PHP_VERSION_ID >= 80600) {
    // Or this test exercised nothing.
    $t->assertTrue("the object's connection is persistent", $persistent);
}

$hold = oxphp_async($fetch, $fixture . 'hold');
$deadline = microtime(true) + 3.0;
while (!isset($sharedState['moves_hold_started']) && microtime(true) < $deadline) {
    oxphp_usleep(50_000);
}
oxphp_usleep(200_000);

$move = oxphp_async($fetch, $fixture . 'move');
oxphp_usleep((int) ($holdFor * 1_000_000));

// Read here, before the release: once the holder is let go, either flag set
// afterwards says nothing about this window.
$holderParked = isset($sharedState['moves_hold_started']) && !isset($sharedState['moves_hold_done']);
$moveStarted = isset($sharedState['moves_move_started']);
$moveStillWaiting = $moveStarted && !isset($sharedState['moves_move_done']);

$release = new Redis();
$release->connect($host, 6379, 3.0);
$release->rPush('hooksdb:moves:list', 'released');
$release->close();

$holdBody = oxphp_async_await($hold);
$moveBody = oxphp_async_await($move);
$held = $decode($holdBody, 'hold-done:');
$moved = $decode($moveBody, 'move-done:');
$t->meta('hold_body', $holdBody);
$t->meta('move_body', $moveBody);

$t->assertTrue('the holder was parked on its pop while the move ran', $holderParked);
$t->assertTrue('the move had started before the holder was released', $moveStarted);

$t->assertSame('the holder got the answer to its own pop', $held['popped'] ?? $holdBody,
    ['hooksdb:moves:list', 'released']);
$t->assertSame('the move connected', $moved['moved'] ?? $moveBody, true);
$t->assertTrue('the move was still waiting at the release', $moveStillWaiting);
$t->assertGreaterThan("and it took the holder's turn to finish", (float) ($moved['seconds'] ?? 0.0),
    $holdFor * 0.8);

$t->done();
