<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('redis_task_multiplex', 'hooksdb');

// The mysqlnd tests cover a client bundled with PHP; phpredis is an independent
// extension, and it reaching the same hook is what shows the coverage follows
// from using php_streams rather than from anything specific to mysqlnd.
//
// BLPOP on an empty list is the right primitive here: Redis serves commands on
// one thread but parks blocked clients instead of holding that thread, so three
// concurrent BLPOPs all return after their own timeout. Anything that made the
// server itself wait (DEBUG SLEEP) would serialize server-side and measure
// nothing about the client.
$host = getenv('DB_REDIS_HOST') ?: 'hooksdb-redis';

$tasks = [];
for ($i = 0; $i < 3; $i++) {
    $tasks[] = oxphp_async(function () use ($host, $i): array {
        $redis = new Redis();
        // The explicit read timeout must exceed the BLPOP timeout, otherwise
        // the socket gives up first and the failure looks like a hook problem.
        $redis->connect($host, 6379, 3.0, null, 0, 5.0);

        $started = microtime(true);
        $popped = $redis->blPop(["hooksdb:empty:{$i}"], 1);
        $finished = microtime(true);
        $redis->close();

        return ['popped' => $popped, 'started' => $started, 'finished' => $finished];
    });
}
$results = oxphp_async_await_all($tasks);

$span = max(array_column($results, 'finished')) - min(array_column($results, 'started'));

// Keyed by task id, so it is re-indexed to keep the labels below positional.
foreach (array_values($results) as $i => $r) {
    $t->assertTrue(
        "task {$i} timed out on an empty list rather than popping a value",
        $r['popped'] === false || $r['popped'] === []
    );
    $t->assertGreaterThan(
        "task {$i} really waited for the server",
        $r['finished'] - $r['started'],
        0.9
    );
}

$t->assertTrue(
    'the three BLPOPs overlapped on one async worker (span < 2.0s, serial would be >= 3s)',
    $span < 2.0
);

$t->done();
