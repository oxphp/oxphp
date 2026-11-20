<?php

declare(strict_types=1);

// One side of test_shared_pconnect_two_objects, chosen by ?role=, on a connection
// opened the way ?round= says:
//
//   pconnect      — pconnect() with a persistent id;
//   pconnect-long — the same;
//   ctor     — new Redis() with the `persistent` option, which connects lazily,
//              inside the first command.
//
// With redis.pconnect.pooling_enabled=0 a phpredis build that keeps persistent
// connections under ZTS hands every object with the same arguments the one stream
// it registered for them, so the holder and the waiter end up on one socket. Each
// has an object of its own: what is shared is the connection, not the PHP value.
//
//   open — opens the second object and leaves it in $sharedState, the way a worker
//          keeps a client it opened at boot: the request that opened it is over,
//          and with it anything that request held, before the holder starts. The
//          pconnect round passes the port as a numeric string, as an application
//          reading it from the environment does, and the pconnect-long round as an
//          int; phpredis names the connection by the number either way;
//   hold — sends a CLIENT ID, then blocks on an empty list until the test pushes a
//          value onto it, so the request stays parked in its read;
//   wait — takes the second object: reads a key the test seeded, then its CLIENT
//          ID. The object is destroyed with this request, where pooling is off:
//          destroyed where it is on, phpredis would put the shared stream in its
//          pool.
//
// Not a TestCase — the body is read by the request that started this one, so a
// failure has to arrive as text rather than as an exception page.
require_once __DIR__ . '/redis_pconnect_weak.php';

$host = getenv('DB_REDIS_HOST') ?: 'hooksdb-redis';
$role = $_GET['role'] ?? '';
$round = $_GET['round'] ?? '';

ini_set('redis.pconnect.pooling_enabled', '0');

$open = static function () use ($host, $round): Redis {
    // Read timeout above the BLPOP timeout, or the socket gives up first.
    if ($round === 'pconnect' || $round === 'pconnect-long') {
        $redis = new Redis();
        $redis->pconnect($host, 6379, 3.0, 'hooksdb-two-objects', 0, 15.0);

        return $redis;
    }

    return new Redis([
        'host' => $host,
        'port' => 6379,
        'connectTimeout' => 3.0,
        'readTimeout' => 15.0,
        'persistent' => true,
    ]);
};

try {
    if ($role === 'hold') {
        $redis = $open();
        $id = $redis->rawCommand('CLIENT', 'ID');
        $sharedState["two_objects_{$round}_hold_started"] = true;

        // Printed verbatim: phpredis turns a reply meant for someone else into a
        // falsy result, so accepting anything falsy would accept the defect.
        $popped = $redis->blPop(["hooksdb:two-objects:{$round}"], 12);
        $sharedState["two_objects_{$round}_hold_done"] = true;
        echo 'hold-done:' . json_encode([
            'client_id' => $id,
            'popped' => $popped,
            'persistent_id' => $redis->getPersistentID(),
        ]);
    } elseif ($role === 'open') {
        if ($round === 'pconnect') {
            $redis = new Redis();
            oxphp_test_redis_pconnect_weak($redis, $host, '6379', 3.0, 'hooksdb-two-objects', 15.0);
        } else {
            $redis = $open();
        }
        $sharedState["two_objects_{$round}_waiter"] = $redis;
        echo 'open-done';
    } elseif ($role === 'wait') {
        $redis = $sharedState["two_objects_{$round}_waiter"] ?? null;
        unset($sharedState["two_objects_{$round}_waiter"]);
        if (!$redis instanceof Redis) {
            echo 'fixture-failed:no object left by the open role';
            return;
        }
        $sharedState["two_objects_{$round}_wait_started"] = true;
        $started = microtime(true);
        $value = $redis->get('hooksdb:two-objects:probe');
        $seconds = microtime(true) - $started;
        $id = $redis->rawCommand('CLIENT', 'ID');
        $sharedState["two_objects_{$round}_wait_done"] = true;
        echo 'wait-done:' . json_encode(['client_id' => $id, 'value' => $value, 'seconds' => round($seconds, 3)]);
    } else {
        echo 'fixture-failed:unknown role';
    }
} catch (\Throwable $e) {
    echo 'fixture-failed:' . get_class($e) . ': ' . str_replace("\n", ' ', $e->getMessage());
}
