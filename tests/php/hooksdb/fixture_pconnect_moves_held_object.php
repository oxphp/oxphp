<?php

declare(strict_types=1);

// One side of test_shared_pconnect_moves_held_object, chosen by ?role=, all on one
// Redis object kept in $sharedState:
//
//   open — opens it with pconnect() under one persistent id;
//   hold — blocks on an empty list through it until the test pushes a value onto
//          that list, so the request stays parked in its read;
//   move — calls pconnect() on it under another persistent id, which moves the
//          object off the connection the holder is reading its reply from.
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
        $redis = new Redis();
        // Read timeout above the BLPOP timeout, or the socket gives up first. Both
        // short, so a holder whose read went wrong is done inside the runner's limit.
        $redis->pconnect($host, 6379, 3.0, 'hooksdb-moves-a', 0, 6.0);
        $sharedState['moves_object'] = $redis;
        echo 'open-done:' . json_encode(['persistent_id' => $redis->getPersistentID()]);
    } elseif ($role === 'hold') {
        $redis = $sharedState['moves_object'] ?? null;
        if (!$redis instanceof Redis) {
            echo 'fixture-failed:no object left by the open role';
            return;
        }
        $sharedState['moves_hold_started'] = true;
        // Printed verbatim, so a pop that came back as anything but its own answer
        // shows as what it was.
        $popped = $redis->blPop(['hooksdb:moves:list'], 5);
        $sharedState['moves_hold_done'] = true;
        echo 'hold-done:' . json_encode(['popped' => $popped]);
    } elseif ($role === 'move') {
        $redis = $sharedState['moves_object'] ?? null;
        unset($sharedState['moves_object']);
        if (!$redis instanceof Redis) {
            echo 'fixture-failed:no object left by the open role';
            return;
        }
        $sharedState['moves_move_started'] = true;
        $started = microtime(true);
        $moved = $redis->pconnect($host, 6379, 3.0, 'hooksdb-moves-b', 0, 6.0);
        $seconds = microtime(true) - $started;
        $sharedState['moves_move_done'] = true;
        echo 'move-done:' . json_encode(['moved' => $moved, 'seconds' => round($seconds, 3)]);
    } else {
        echo 'fixture-failed:unknown role';
    }
} catch (\Throwable $e) {
    echo 'fixture-failed:' . get_class($e) . ': ' . str_replace("\n", ' ', $e->getMessage());
}
