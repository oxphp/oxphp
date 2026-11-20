<?php

declare(strict_types=1);

// One side of test_shared_redis_reconnect_while_waited, chosen by ?role=, all on one
// persistent Redis object kept in $sharedState:
//
//   open — builds it, persistent under its own id, with a one-second read timeout;
//   hold — blocks on an empty list through it for longer than that timeout, so the
//          read gives up and phpredis closes the connection; then pops again, which
//          reconnects under the same id, and stays parked until the test pushes a
//          value onto the second list;
//   wait — reads a key through it, started while the first pop is still parked.
//
// Not a TestCase — the body is read by the request that started this one, so a
// failure has to arrive as text rather than as an exception page.
$host = getenv('DB_REDIS_HOST') ?: 'hooksdb-redis';
$role = $_GET['role'] ?? '';

// Pooling off for every role, including the one that ends up dropping the object:
// dropped where pooling is on, phpredis would put its stream in its pool.
ini_set('redis.pconnect.pooling_enabled', '0');

try {
    if ($role === 'open') {
        $sharedState['reconnect_object'] = new Redis([
            'host' => $host,
            'port' => 6379,
            'connectTimeout' => 3.0,
            'readTimeout' => 1.0,
            'persistent' => 'hooksdb-reconnect',
        ]);
        echo 'open-done';
    } elseif ($role === 'hold') {
        $redis = $sharedState['reconnect_object'] ?? null;
        if (!$redis instanceof Redis) {
            echo 'fixture-failed:no object left by the open role';
            return;
        }
        $sharedState['reconnect_hold_started'] = true;
        $lost = null;
        try {
            $redis->blPop(['hooksdb:reconnect:first'], 5);
        } catch (\RedisException $e) {
            $lost = get_class($e) . ': ' . $e->getMessage();
        }
        // Read here, while the read that lost the connection is the last thing
        // this request did: whether the other request was waiting for it then.
        $sharedState['reconnect_wait_pending_at_loss'] = isset($sharedState['reconnect_wait_started'])
            && !isset($sharedState['reconnect_wait_done']);
        $persistentId = $redis->getPersistentID();

        $redis->setOption(Redis::OPT_READ_TIMEOUT, 6.0);
        $sharedState['reconnect_retry_started'] = true;
        $popped = $redis->blPop(['hooksdb:reconnect:second'], 4);
        $sharedState['reconnect_hold_done'] = true;
        echo 'hold-done:' . json_encode([
            'lost' => $lost,
            'persistent_id' => $persistentId,
            'popped' => $popped,
        ]);
    } elseif ($role === 'wait') {
        $redis = $sharedState['reconnect_object'] ?? null;
        unset($sharedState['reconnect_object']);
        if (!$redis instanceof Redis) {
            echo 'fixture-failed:no object left by the open role';
            return;
        }
        $sharedState['reconnect_wait_started'] = true;
        $started = microtime(true);
        $value = $redis->get('hooksdb:reconnect:probe');
        $seconds = microtime(true) - $started;
        $sharedState['reconnect_wait_done'] = true;
        echo 'wait-done:' . json_encode(['value' => $value, 'seconds' => round($seconds, 3)]);
    } else {
        echo 'fixture-failed:unknown role';
    }
} catch (\Throwable $e) {
    echo 'fixture-failed:' . get_class($e) . ': ' . str_replace("\n", ' ', $e->getMessage());
}
