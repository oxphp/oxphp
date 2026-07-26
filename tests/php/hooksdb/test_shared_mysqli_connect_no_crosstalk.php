<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('shared_mysqli_connect_no_crosstalk', 'hooksdb');

// mysqli::connect() is not a query, and that is the point. Called on a live
// object it does not open a second connection — it reconnects the link that
// object already holds, which means it takes the socket out from under whichever
// fiber is mid-exchange on it. A command sent into someone else's exchange is
// refused by mysqlnd and the wire survives; a socket replaced under a parked
// fiber leaves it waiting on a descriptor that is gone.
//
// It is a separate method from real_connect() — mysqli::connect() has its own
// entry in the class table — so being covered there says nothing about being
// covered here. The procedural mysqli_connect() is a different call again: it
// builds a new object, so there is nothing of anyone else's to displace, and it
// is deliberately not guarded.
$task = oxphp_async(function (): array {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return ['body' => "connect failed: {$errstr} ({$errno})"];
    }
    stream_set_timeout($sock, 8);
    fwrite($sock, "GET /tests/hooksdb/fixture_shared_mysqli_conn_hold.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body];
});

oxphp_usleep(200_000);

$t->assertTrue(
    'the holding request created the shared link before this one woke',
    isset($sharedState['mysqli_conn'])
);

$error = '';
$started = microtime(true);
try {
    $sharedState['mysqli_conn']->connect(
        getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql',
        getenv('DB_USER') ?: 'appuser',
        getenv('DB_PASS') ?: 'apppass',
        getenv('DB_NAME') ?: 'appdb'
    );
} catch (\Throwable $e) {
    $error = $e->getMessage();
}
$waited = microtime(true) - $started;

// After the wait the link is this fiber's, freshly reconnected, and has to work.
$after = null;
try {
    $result = $sharedState['mysqli_conn']->query('SELECT 42');
    $after = $result->fetch_row()[0];
} catch (\Throwable $e) {
    $error = $error === '' ? $e->getMessage() : $error;
}

$inner = oxphp_async_await($task);

// The load-bearing pair. The wait alone would also be produced by a connect() that
// then broke the holder, and the holder's result alone would also survive a
// connect() that never happened.
$t->assertGreaterThan(
    'the reconnect waited for the fiber holding the link instead of replacing its socket',
    $waited,
    0.5
);
$t->assertContains(
    'the holding request finished with the answer to its own query',
    $inner['body'],
    'mysqli-conn-hold-done:'
);

$t->assertSame('the reconnected link answers this fiber', $after, '42');
$t->assertSame('neither the reconnect nor the query reported an error', $error, '');

$t->done();
