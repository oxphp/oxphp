<?php

declare(strict_types=1);

// The phpredis half of the shared-connection case, and the first request to
// touch the shared Redis connection: it creates the handle the worker keeps for
// its whole life ($sharedState comes from the worker entry, which `include` puts
// in scope) and then blocks on an empty list for a second.
//
// BLPOP is the right primitive: Redis parks a blocked client instead of holding
// its own thread, so the wait belongs to this connection alone. The explicit
// read timeout has to exceed the BLPOP timeout, or the socket gives up first and
// the failure looks like a hook problem.
//
// Not a TestCase — the body is read by the request that started this one, so a
// failure has to arrive as text rather than as an exception page.
try {
    if (!isset($sharedState['redis'])) {
        $redis = new Redis();
        $redis->connect(getenv('DB_REDIS_HOST') ?: 'hooksdb-redis', 6379, 3.0, null, 0, 5.0);
        $sharedState['redis'] = $redis;
    }

    // Printed verbatim rather than reduced to "empty": phpredis turns a reply
    // meant for another fiber into a falsy result, so a test that accepts
    // anything falsy accepts the defect it is meant to catch.
    $popped = $sharedState['redis']->blPop(['hooksdb:shared:empty'], 1);
    echo 'redis-hold-done:' . var_export($popped, true);
} catch (\Throwable $e) {
    echo 'redis-hold-failed:' . $e->getMessage();
}
