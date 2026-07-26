<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('shared_redis_task_multiplex', 'hooksdb');

// The same shared-connection case in the async pool. Task fibers multiplex on
// one async worker thread (ASYNC_WORKERS=1) and share that thread's PHP context,
// so a connection parked in a global is shared between them exactly as a
// worker-boot connection is shared between requests — and a socket read that
// parks one task leaves its exchange half-finished for the next one to walk into.
//
// Dispatch order with one async worker is the queue's order: the holder runs
// first, creates the connection and blocks on an empty list; the prober starts
// while the holder is parked, waits long enough for that to be true, and then
// uses the same connection.
$host = getenv('DB_REDIS_HOST') ?: 'hooksdb-redis';
$key = 'hooksdb:task:probe';

$holder = oxphp_async(function () use ($host): string {
    try {
        if (!isset($GLOBALS['hooksdb_task_redis'])) {
            $redis = new Redis();
            $redis->connect($host, 6379, 3.0, null, 0, 5.0);
            $GLOBALS['hooksdb_task_redis'] = $redis;
        }
        $popped = $GLOBALS['hooksdb_task_redis']->blPop(['hooksdb:task:empty'], 1);

        return ($popped === false || $popped === []) ? 'empty' : json_encode($popped);
    } catch (\Throwable $e) {
        return 'failed:' . $e->getMessage();
    }
});

$prober = oxphp_async(function () use ($key): array {
    oxphp_usleep(200_000);
    if (!isset($GLOBALS['hooksdb_task_redis'])) {
        return ['stored' => null, 'error' => 'the holder task did not share its connection', 'waited' => 0.0];
    }

    $stored = null;
    $error = '';
    $started = microtime(true);
    try {
        $GLOBALS['hooksdb_task_redis']->set($key, 'from-the-second-task');
        $stored = $GLOBALS['hooksdb_task_redis']->get($key);
    } catch (\Throwable $e) {
        $error = $e->getMessage();
    }

    return ['stored' => $stored, 'error' => $error, 'waited' => microtime(true) - $started];
});

$held = oxphp_async_await($holder);
$probe = oxphp_async_await($prober);

$t->assertSame('the second task read back its own value', $probe['stored'], 'from-the-second-task');
$t->assertSame('the shared connection reported no protocol failure', $probe['error'], '');
$t->assertGreaterThan(
    'the second task waited for the task that held the connection instead of cutting in',
    $probe['waited'],
    0.5
);
$t->assertSame(
    'the holding task got the answer to its own BLPOP, not the second task\'s reply',
    $held,
    'empty'
);

$t->done();
