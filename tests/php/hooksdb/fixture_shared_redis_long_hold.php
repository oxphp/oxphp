<?php

declare(strict_types=1);

// Holds the shared Redis connection for longer than its counterpart is willing to
// wait, which is the only way to reach the give-up branch of the claim wait. Under
// its own key so the shorter-lived phpredis tests are unaffected by the delay.
try {
    if (!isset($sharedState['redis_slow'])) {
        $redis = new Redis();
        $redis->connect(getenv('DB_REDIS_HOST') ?: 'hooksdb-redis', 6379, 3.0, null, 0, 10.0);
        $sharedState['redis_slow'] = $redis;
    }

    $popped = $sharedState['redis_slow']->blPop(['hooksdb:slow:empty'], 6);
    echo 'slow-hold-done:' . var_export($popped, true);
} catch (\Throwable $e) {
    echo 'slow-hold-failed:' . str_replace("\n", ' ', $e->getMessage());
}
