<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('shared_pconnect_two_objects', 'hooksdb');

// Two Redis objects opened with identical persistent arguments and used by two
// concurrent requests. Whether they are two connections or one is phpredis's call, and it
// depends on the pooling switch: with pooling on (the default) each live object is
// handed a connection of its own, and with it off a build that keeps persistent
// connections under ZTS gives both the one stream it registered for those
// arguments. One socket under two objects is what a claim keyed by the object
// cannot see, so the second request's command would land inside the first one's
// exchange.
$host = getenv('DB_REDIS_HOST') ?: 'hooksdb-redis';

// Pooling on: one live object, one connection — CLIENT ID being the server's own
// name for a connection.
ini_set('redis.pconnect.pooling_enabled', '1');
$first = new Redis();
$first->pconnect($host, 6379, 3.0, 'premise-1', 0, 5.0);
$second = new Redis();
$second->pconnect($host, 6379, 3.0, 'premise-1', 0, 5.0);
$t->assertNotEqual(
    'two live handles opened with identical pconnect() arguments are two connections'
        . ' (redis.pconnect.pooling_enabled=1)',
    $first->rawCommand('CLIENT', 'ID'),
    $second->rawCommand('CLIENT', 'ID')
);
unset($first, $second);
ini_restore('redis.pconnect.pooling_enabled');

// Pooling off: the scenario. A second object is opened with the same arguments by
// a request that is over before anything else starts; a holder then parks in a
// blocking pop, and another request reads a key through the second object. Both
// must come away with their own answer.
$fetch = static function (string $path): string {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return "connect failed: {$errstr} ({$errno})";
    }
    stream_set_timeout($sock, 20);
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

$seed = new Redis();
$seed->connect($host, 6379, 3.0);
$seed->set('hooksdb:two-objects:probe', 'two-objects-probe-value');
$seed->del('hooksdb:two-objects:pconnect', 'hooksdb:two-objects:pconnect-long', 'hooksdb:two-objects:ctor');
$seed->close();

// How long the waiter is left alone with the holder parked before the holder is
// released. A waiter on its own connection answers in milliseconds; one sharing
// the holder's has to sit this out, so its time is the measure of which it was.
$holdFor = 0.5;

// The pconnect() rounds name the connection by a persistent id — the second object
// passing its port once as a numeric string and once as an int — the constructor
// round by the connect timeout, and the constructor round also connects inside the
// first command rather than in a connect call of its own.
foreach (['pconnect', 'pconnect-long', 'ctor'] as $round) {
    foreach (['hold_started', 'hold_done', 'wait_started', 'wait_done', 'waiter'] as $edge) {
        unset($sharedState["two_objects_{$round}_{$edge}"]);
    }

    $fixture = "/tests/hooksdb/fixture_pconnect_two_objects.php?round={$round}&role=";
    $t->assertSame("{$round}: the second object was opened ahead of the holder",
        oxphp_async_await(oxphp_async($fetch, $fixture . 'open')), 'open-done');

    $hold = oxphp_async($fetch, $fixture . 'hold');
    $deadline = microtime(true) + 3.0;
    while (!isset($sharedState["two_objects_{$round}_hold_started"]) && microtime(true) < $deadline) {
        oxphp_usleep(50_000);
    }
    oxphp_usleep(200_000);

    $wait = oxphp_async($fetch, $fixture . 'wait');
    oxphp_usleep((int) ($holdFor * 1_000_000));

    // Read here, before the release: once the holder is let go, either flag set
    // afterwards says nothing about this window.
    $holderParked = isset($sharedState["two_objects_{$round}_hold_started"])
        && !isset($sharedState["two_objects_{$round}_hold_done"]);
    $waiterStarted = isset($sharedState["two_objects_{$round}_wait_started"]);
    $waiterStillWaiting = $waiterStarted && !isset($sharedState["two_objects_{$round}_wait_done"]);

    $release = new Redis();
    $release->connect($host, 6379, 3.0);
    $release->rPush("hooksdb:two-objects:{$round}", 'released');
    $release->close();

    $holdBody = oxphp_async_await($hold);
    $waitBody = oxphp_async_await($wait);
    $held = $decode($holdBody, 'hold-done:');
    $waited = $decode($waitBody, 'wait-done:');
    $t->meta("{$round}_hold_body", $holdBody);
    $t->meta("{$round}_wait_body", $waitBody);

    $t->assertTrue("{$round}: the holder was parked on its pop while the waiter ran", $holderParked);
    $t->assertTrue("{$round}: the waiter had started before the holder was released", $waiterStarted);

    $shared = $held !== null && $waited !== null && $held['client_id'] === $waited['client_id'];
    $t->meta("{$round}_shared", $shared);
    // Whether the connection is persistent is phpredis's call too. Under ZTS 6.3.0
    // turns pconnect() into a plain connect, which getPersistentID() then reports by
    // answering null; the develop commit the fixture installs on PHP 8.6 does not,
    // and the constructor's option is kept persistent by both. A persistent
    // connection opened with the same arguments is the one both objects get.
    $persistent = $round === 'ctor' || ($held['persistent_id'] ?? null) === 'hooksdb-two-objects';
    $t->meta("{$round}_persistent", $persistent);
    if (PHP_VERSION_ID >= 80600) {
        // Or this round exercised nothing.
        $t->assertTrue("{$round}: the holder's connection is persistent", $persistent);
    }
    $t->assertSame("{$round}: the two objects are on one connection exactly when it is persistent",
        $shared, $persistent);

    $t->assertSame("{$round}: the holder got the answer to its own pop", $held['popped'] ?? $holdBody,
        ["hooksdb:two-objects:{$round}", 'released']);
    $t->assertSame("{$round}: the waiter got its own key", $waited['value'] ?? $waitBody,
        'two-objects-probe-value');

    $seconds = (float) ($waited['seconds'] ?? 99.0);
    if ($shared) {
        // Its command went out only once the holder's request was over, rather
        // than into the middle of the holder's exchange.
        $t->assertTrue("{$round}: the waiter on the shared connection was still waiting at the release",
            $waiterStillWaiting);
        $t->assertGreaterThan("{$round}: and its call took the holder's turn to finish", $seconds, $holdFor * 0.8);
    } else {
        $t->assertLessThan("{$round}: a waiter on a connection of its own did not wait", $seconds, $holdFor * 0.8);
    }
}

$t->done();
