<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('mysql_request_multiplex', 'hooksdb');

// PHP_WORKERS=1, so both database waits in this test belong to the same worker
// thread: this request fiber's own query, and the one made by a second request
// fiber serving fixture_db_sleep.php. A hooked read parks whichever fiber is
// waiting and lets the worker run the other; a blocking read pins the worker
// inside recv() and the two queries can only run one after the other.
//
// This covers the HTTP scheduler's readiness pass, whereas
// test_mysql_task_multiplex covers the async-task scheduler.
$started = microtime(true);

$task = oxphp_async(function (): array {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return ['body' => "connect failed: {$errstr} ({$errno})"];
    }
    stream_set_timeout($sock, 8);
    fwrite($sock, "GET /tests/hooksdb/fixture_db_sleep.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body];
});

// Give the inner request time to reach the worker and enter its own wait, so
// that the two waits genuinely overlap rather than merely following each other.
usleep(150_000);

$pdo = new PDO(
    'mysql:host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql')
        . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb'),
    getenv('DB_USER') ?: 'appuser',
    getenv('DB_PASS') ?: 'apppass',
    [PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION]
);

$queryStarted = microtime(true);
$pdo->query('SELECT SLEEP(1)')->fetchColumn();
$ownWait = microtime(true) - $queryStarted;

// Keyed by task id, not by position, so the single result is taken by value.
$inner = array_values(oxphp_async_await_all([$task]))[0];
$total = microtime(true) - $started;

$t->assertContains('the inner request was served while this fiber waited', $inner['body'], 'db-done');
$t->assertGreaterThan('this fiber really waited for its own query', $ownWait, 0.9);
$t->assertTrue(
    'both queries ran on one worker at once (total < 1.8s, serial would be >= 2.1s)',
    $total < 1.8
);

$t->done();
