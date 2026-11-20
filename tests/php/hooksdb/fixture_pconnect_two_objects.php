<?php

declare(strict_types=1);

// One side of test_shared_pconnect_two_objects, chosen by ?role=, on a connection
// opened the way ?round= says:
//
//   pconnect      — pconnect() with a persistent id;
//   pconnect-long — the same;
//   pconnect-timeout — the second object from pconnect() with no persistent id, the
//              holder from the constructor's `persistent` option set to true, so
//              phpredis names both by the connect timeout — one no other round
//              connects with;
//   ctor     — new Redis() with the `persistent` option, which connects lazily,
//              inside the first command;
//   mixed    — the holder from the constructor, the second object from pconnect(),
//              under one persistent id. The constructor's option is persistent on
//              every phpredis; pconnect() is not on 6.3.0 under ZTS, which then
//              gives the second object a socket of its own;
//   mixed-late — the same, with the constructor's stream registered only after
//              the second object's pconnect(), by the holder's first command;
//   pooled   — both from pconnect() with pooling on, which gives each its own
//              connection, beside a stream an earlier object left registered under
//              their id with pooling off;
//   pooled-ctor — the same, both from the constructor;
//   pooled-late — both from the constructor, the second object built with pooling
//              off and first used with it on, which takes it a connection of its
//              own from the pool;
//   unpooled-late — the other way round: built with pooling on and first used
//              with it off, which puts it on the holder's stream;
//   ctor-connect — both from the constructor, the second object then given a
//              connect(), which opens it a plain socket in place of the stream
//              the constructor named;
//   error    — both from the constructor, the second object then given a connect()
//              that phpredis rejects for its retry interval before it lets go of
//              anything.
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
//          int; phpredis names the connection by the number either way. The mixed
//          and pooled rounds first connect an object of their own under the id and
//          drop it, with pooling off, which leaves its stream registered. The
//          pooled-late and unpooled-late rounds only build the object: phpredis
//          connects it in the wait role, under that role's setting;
//   hold — sends a CLIENT ID, then blocks on an empty list until the test pushes a
//          value onto it, so the request stays parked in its read;
//   wait — takes the second object: reads a key the test seeded, then its CLIENT
//          ID. The object is destroyed with this request, where pooling is off:
//          destroyed where it is on, phpredis would put the shared stream in its
//          pool. The pooled rounds' objects, pooled-late's included, are on pooled
//          connections already.
//
// Not a TestCase — the body is read by the request that started this one, so a
// failure has to arrive as text rather than as an exception page.
require_once __DIR__ . '/redis_pconnect_weak.php';

$host = getenv('DB_REDIS_HOST') ?: 'hooksdb-redis';
$role = $_GET['role'] ?? '';
$round = $_GET['round'] ?? '';
$pid = match ($round) {
    'mixed', 'mixed-late', 'pooled', 'pooled-ctor', 'pooled-late', 'unpooled-late',
    'ctor-connect', 'error' => "hooksdb-two-objects-{$round}",
    default => 'hooksdb-two-objects',
};
$pooled = $round === 'pooled' || $round === 'pooled-ctor';

ini_set('redis.pconnect.pooling_enabled', '0');

// Read timeout above the BLPOP timeout, or the socket gives up first.
$pconnect = static function () use ($host, $pid): Redis {
    $redis = new Redis();
    $redis->pconnect($host, 6379, 3.0, $pid, 0, 15.0);

    return $redis;
};
$construct = static function (bool|string $persistent, float $timeout = 3.0) use ($host): Redis {
    return new Redis([
        'host' => $host,
        'port' => 6379,
        'connectTimeout' => $timeout,
        'readTimeout' => 15.0,
        'persistent' => $persistent,
    ]);
};

try {
    if ($role === 'hold') {
        if ($pooled) {
            ini_set('redis.pconnect.pooling_enabled', '1');
        }
        $redis = match ($round) {
            'pconnect', 'pconnect-long', 'pooled' => $pconnect(),
            'ctor' => $construct(true),
            'pconnect-timeout' => $construct(true, 2.5),
            default => $construct($pid),
        };
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
        if ($round === 'mixed' || $pooled) {
            $earlier = $construct($pid);
            $earlier->ping();
            unset($earlier);
        }
        if ($pooled) {
            ini_set('redis.pconnect.pooling_enabled', '1');
        }
        if ($round === 'pconnect') {
            $redis = new Redis();
            oxphp_test_redis_pconnect_weak($redis, $host, '6379', 3.0, $pid, 15.0);
        } elseif ($round === 'pconnect-timeout') {
            $redis = new Redis();
            $redis->pconnect($host, 6379, 2.5);
        } elseif ($round === 'ctor') {
            $redis = $construct(true);
        } elseif ($round === 'pooled-ctor' || $round === 'pooled-late') {
            $redis = $construct($pid);
        } elseif ($round === 'unpooled-late') {
            ini_set('redis.pconnect.pooling_enabled', '1');
            $redis = $construct($pid);
        } elseif ($round === 'ctor-connect') {
            $redis = $construct($pid);
            $redis->connect($host, 6379, 3.0);
        } elseif ($round === 'error') {
            $redis = $construct($pid);
            try {
                $redis->connect($host, 6379, 3.0, null, -1);
                echo 'fixture-failed:connect() took a negative retry interval';
                return;
            } catch (\ValueError) {
            }
        } else {
            $redis = $pconnect();
        }
        $sharedState["two_objects_{$round}_waiter"] = $redis;
        echo 'open-done';
    } elseif ($role === 'wait') {
        if ($pooled || $round === 'pooled-late') {
            ini_set('redis.pconnect.pooling_enabled', '1');
        }
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
        echo 'wait-done:' . json_encode([
            'client_id' => $id,
            'value' => $value,
            'seconds' => round($seconds, 3),
            'persistent_id' => $redis->getPersistentID(),
        ]);
    } else {
        echo 'fixture-failed:unknown role';
    }
} catch (\Throwable $e) {
    echo 'fixture-failed:' . get_class($e) . ': ' . str_replace("\n", ' ', $e->getMessage());
}
