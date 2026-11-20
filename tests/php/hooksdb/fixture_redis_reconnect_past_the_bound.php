<?php

declare(strict_types=1);

// One side of test_shared_redis_reconnect_past_the_bound, chosen by ?role=, all on
// one persistent Redis object kept in $sharedState:
//
//   open  — builds it, persistent under its own id, with a read timeout well
//           inside the two seconds this image bounds a wait for a connection at;
//   cycle — blocks on an empty list through it four times running; each read
//           times out, phpredis closes the connection, and the next pop
//           reconnects under the same id to a new one;
//   wait  — reads a key through it, started while the first pop is parked.
//
// Not a TestCase — the body is read by the request that started this one, so a
// failure has to arrive as text rather than as an exception page.
$host = getenv('DB_REDIS_HOST') ?: 'hooksdb-redis';
$role = $_GET['role'] ?? '';

ini_set('redis.pconnect.pooling_enabled', '0');

try {
    if ($role === 'open') {
        $sharedState['bound_object'] = new Redis([
            'host' => $host,
            'port' => 6379,
            'connectTimeout' => 3.0,
            'readTimeout' => 0.9,
            'persistent' => 'hooksdb-reconnect-bound',
        ]);
        echo 'open-done';
    } elseif ($role === 'cycle') {
        $redis = $sharedState['bound_object'] ?? null;
        if (!$redis instanceof Redis) {
            echo 'fixture-failed:no object left by the open role';
            return;
        }
        $sharedState['bound_cycle_started'] = true;
        $sharedState['bound_losses_while_waited'] = 0;
        $losses = [];
        for ($i = 0; $i < 4; $i++) {
            try {
                $redis->blPop(['hooksdb:bound:empty'], 5);
                $losses[] = 'no loss';
            } catch (\RedisException $e) {
                $losses[] = get_class($e);
            }
            // Read at each loss: whether the other request was waiting through it.
            if (isset($sharedState['bound_wait_started']) && !isset($sharedState['bound_wait_done'])) {
                $sharedState['bound_losses_while_waited']++;
            }
        }
        $sharedState['bound_cycle_done'] = true;
        echo 'cycle-done:' . json_encode(['losses' => $losses]);
    } elseif ($role === 'wait') {
        $redis = $sharedState['bound_object'] ?? null;
        unset($sharedState['bound_object']);
        if (!$redis instanceof Redis) {
            echo 'fixture-failed:no object left by the open role';
            return;
        }
        $sharedState['bound_wait_started'] = true;
        $started = microtime(true);
        try {
            $outcome = 'value:' . var_export($redis->get('hooksdb:bound:probe'), true);
        } catch (\RedisException $e) {
            $outcome = get_class($e) . ': ' . $e->getMessage();
        }
        $seconds = microtime(true) - $started;
        // Read before anything else this request does: whether the call ended while
        // the other request was still replacing the connection under it.
        $cycling = isset($sharedState['bound_cycle_started']) && !isset($sharedState['bound_cycle_done']);
        $sharedState['bound_wait_done'] = true;
        echo 'wait-done:' . json_encode([
            'outcome' => $outcome,
            'seconds' => round($seconds, 3),
            'cycling' => $cycling,
        ]);
    } else {
        echo 'fixture-failed:unknown role';
    }
} catch (\Throwable $e) {
    echo 'fixture-failed:' . get_class($e) . ': ' . str_replace("\n", ' ', $e->getMessage());
}
