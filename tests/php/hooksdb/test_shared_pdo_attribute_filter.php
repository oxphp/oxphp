<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('shared_pdo_attribute_filter', 'hooksdb');

// PDO::getAttribute() is two different calls wearing one name. Some attributes it
// answers out of its own handle and the driver is never called; the rest it passes
// down, and for pdo_mysql that is a command on the wire. Only the second kind may
// be made to wait for the fiber holding the connection.
//
// Both directions are asserted, on one connection inside one window, because each
// half alone can pass for the wrong reason: "it did not wait" is also true when
// nothing was holding the connection, and "it waited" is also true when everything
// waits. Which attribute belongs on which side is decided by PHP's own source, and
// this is what keeps that reading honest against the build actually in use rather
// than the one that was read once:
//
//   ATTR_DRIVER_NAME  — returned from pdo_dbh_t by PDO itself, no driver call
//   ATTR_SERVER_INFO  — pdo_mysql answers it with mysqlnd_stat(), i.e. a command
$task = oxphp_async(function (): array {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return ['body' => "connect failed: {$errstr} ({$errno})"];
    }
    stream_set_timeout($sock, 8);
    fwrite($sock, "GET /tests/hooksdb/fixture_shared_db_attr_hold.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body];
});

oxphp_usleep(200_000);

$t->assertTrue(
    'the holding request created the shared connection before this one woke',
    isset($sharedState['pdo_attr'])
);

$localStarted = microtime(true);
$driver = $sharedState['pdo_attr']->getAttribute(PDO::ATTR_DRIVER_NAME);
$localWaited = microtime(true) - $localStarted;

// Second, because it takes the connection: everything after it is this fiber's.
$wireStarted = microtime(true);
$serverInfo = $sharedState['pdo_attr']->getAttribute(PDO::ATTR_SERVER_INFO);
$wireWaited = microtime(true) - $wireStarted;

$inner = oxphp_async_await($task);

$t->assertSame('an attribute PDO answers by itself came back', $driver, 'mysql');
$t->assertLessThan(
    'reading it did not wait for the fiber holding the connection',
    $localWaited,
    0.2
);

$t->assertType('an attribute that reaches the driver came back', $serverInfo, 'string');
$t->assertGreaterThan(
    'reading it did wait for the fiber holding the connection, as a command must',
    $wireWaited,
    0.5
);

$t->assertContains(
    'the holding request finished with the answer to its own query',
    $inner['body'],
    'attr-hold-done:'
);

$t->done();
