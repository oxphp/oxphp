<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('persistent_dtor_under_streaming_holder', 'hooksdb');

// The destructor's driver cleanup at its most expensive. For pdo_mysql that
// cleanup frees the result the connection is currently reading, on the grounds
// that the next script must not see it — but with a pooled connection shared
// between requests, the result belongs to a request parked in the middle of
// reading it. When that request resumes it writes into freed memory: its rows
// come back wrong, or the worker goes down.
$sharedState['dtor_stream_key'] = 'dtor-stream-' . bin2hex(random_bytes(4));
unset($sharedState['dtor_stream_start'], $sharedState['dtor_stream_end']);

$dsn = 'mysql:host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql')
    . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb');
$user = getenv('DB_USER') ?: 'appuser';
$pass = getenv('DB_PASS') ?: 'apppass';

$task = oxphp_async(function (): array {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return ['body' => "connect failed: {$errstr} ({$errno})"];
    }
    stream_set_timeout($sock, 15);
    fwrite($sock, "GET /tests/hooksdb/fixture_persistent_dtor_stream_hold.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body];
});

$deadline = microtime(true) + 3.0;
while (!isset($sharedState['dtor_stream_start']) && microtime(true) < $deadline) {
    oxphp_usleep(20_000);
}
$t->assertTrue('the holding request started reading before this one woke', isset($sharedState['dtor_stream_start']));

// Far enough into the read that rows are arriving — the first comes after one
// SLEEP — and far enough from its end, about two seconds in, that it is still
// going.
oxphp_usleep(600_000);

$error = '';
$droppedAt = null;
$keep = null;
try {
    $opts = [
        PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
        PDO::ATTR_PERSISTENT => $sharedState['dtor_stream_key'],
    ];
    $pdo = new PDO($dsn, $user, $pass, $opts);
    $keep = new PDO($dsn, $user, $pass, $opts);
    unset($pdo);
    $droppedAt = microtime(true);
} catch (\Throwable $e) {
    $error = str_replace("\n", ' ', $e->getMessage());
}
$t->assertSame('this request built its handles: ' . $error, $error, '');

$body = oxphp_async_await($task)['body'];

$t->assertContains(
    'the holding request read its whole result, row for row',
    $body,
    'persistent-dtor-stream-done: rows:40 intact:true '
);

// The premise, both edges of it: the handle was dropped after the read had been
// going for a while, and before it finished.
$start = $sharedState['dtor_stream_start'] ?? INF;
$end = $sharedState['dtor_stream_end'] ?? -INF;
$t->assertTrue(
    'and the handle was dropped while that read was under way',
    $droppedAt !== null && $droppedAt > $start + 0.2 && $droppedAt < $end
);

preg_match('/ id:(\d+)$/', $body, $m);
$mine = $keep !== null ? (string) $keep->query('SELECT CONNECTION_ID()')->fetchColumn() : '';
$t->assertSame(
    'and was one on the holder\'s connection',
    $mine,
    $m[1] ?? 'the holder printed no connection id'
);

$t->done();
