<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('shared_mysqli_no_crosstalk', 'hooksdb');

// The same shared-connection case as test_shared_mysql_no_crosstalk, reached
// through mysqli's procedural functions instead of PDO's methods. Worth running
// both: they are separate entry points into the same client, and the procedural
// form — the one WordPress uses — passes the connection as an argument rather
// than as $this.
//
// What makes this case different from the phpredis ones is where it fails
// without a guard. mysqlnd tracks its own connection state and refuses a command
// issued while the connection is mid-exchange, before sending anything, so the
// second fiber never reaches the socket: no stream-level guard can see it, and
// the failure is a client error rather than crossed replies.
$task = oxphp_async(function (): array {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return ['body' => "connect failed: {$errstr} ({$errno})"];
    }
    stream_set_timeout($sock, 8);
    fwrite($sock, "GET /tests/hooksdb/fixture_shared_mysqli_hold.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body];
});

oxphp_usleep(200_000);

$t->assertTrue(
    'the holding request created the shared connection before this one woke',
    isset($sharedState['mysqli'])
);

$value = null;
$error = '';
$started = microtime(true);
try {
    $result = mysqli_query($sharedState['mysqli'], 'SELECT 42');
    $value = mysqli_fetch_row($result)[0];
} catch (\Throwable $e) {
    $error = $e->getMessage();
}
$waited = microtime(true) - $started;

$inner = oxphp_async_await($task);

// mysqlnd returns an integer column as a PHP string here (no native types on the
// mysqli path), so this compares loosely on purpose.
$t->assertEqual('this fiber got the answer to its own query', $value, 42);
$t->assertSame('the shared connection reported no client-side protocol failure', $error, '');
$t->assertGreaterThan(
    'the query waited for the fiber that held the connection instead of cutting in',
    $waited,
    0.5
);
$t->assertContains(
    'the holding request finished with the answer to its own query',
    $inner['body'],
    'mysqli-hold-done:'
);

$t->done();
