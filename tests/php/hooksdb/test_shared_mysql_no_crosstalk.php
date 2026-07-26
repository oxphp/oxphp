<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('shared_mysql_no_crosstalk', 'hooksdb');

// One MySQL connection, two request fibers on one worker (PHP_WORKERS=1) — the
// shape every worker-mode application has, since WordPress, Laravel and Symfony
// open their connection when the worker boots and hand the same one to every
// request. A socket read that parks the fiber puts the holder's exchange on hold
// half-finished, and this request then reaches the same connection.
//
// Order is guaranteed by construction rather than by timing: a worker switches
// fibers only at a suspend point, so the request fired below cannot run until
// this one parks, and this one cannot touch the connection until it wakes.
//
// 1. fire fixture_shared_db_hold.php — it creates the shared handle and holds a
//    one-second query open on it
// 2. park here long enough for that to happen (oxphp_usleep suspends in worker
//    mode unconditionally, so this does not depend on the sleep hook)
// 3. run a trivial query on the same connection and see what comes back
$task = oxphp_async(function (): array {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return ['body' => "connect failed: {$errstr} ({$errno})"];
    }
    stream_set_timeout($sock, 8);
    fwrite($sock, "GET /tests/hooksdb/fixture_shared_db_hold.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body];
});

oxphp_usleep(200_000);

$t->assertTrue(
    'the holding request created the shared connection before this one woke',
    isset($sharedState['pdo'])
);

$value = null;
$error = '';
$started = microtime(true);
try {
    $value = $sharedState['pdo']->query('SELECT 42')->fetchColumn();
} catch (\Throwable $e) {
    $error = $e->getMessage();
}
$waited = microtime(true) - $started;

$inner = oxphp_async_await($task);

// Both halves of the guarantee. The result alone can be satisfied by the two
// queries never overlapping, and the wait alone says nothing about the answer.
$t->assertSame('this fiber got the answer to its own query', $value, 42);
$t->assertSame('the shared connection reported no client-side protocol failure', $error, '');
$t->assertGreaterThan(
    'the query waited for the fiber that held the connection instead of cutting in',
    $waited,
    0.5
);
$t->assertContains(
    'the holding request finished with the answer to its own query',
    $inner['body'],
    'hold-done:0'
);

$t->done();
