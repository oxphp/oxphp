<?php

// Without strict_types on purpose: pconnect() takes a port passed as a numeric
// string only from a file that does not declare them, which is how an application
// that reads its port from the environment calls it.

function oxphp_test_redis_pconnect_weak(
    Redis $redis,
    string $host,
    string $port,
    float $timeout,
    string $persistentId,
    float $readTimeout
): bool {
    return $redis->pconnect($host, $port, $timeout, $persistentId, 0, $readTimeout);
}
