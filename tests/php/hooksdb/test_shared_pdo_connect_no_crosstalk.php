<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('shared_pdo_connect_no_crosstalk', 'hooksdb');

// The shared-connection case again, on a connection opened with PDO::connect().
// It is worth its own test because the object it returns is not a PDO: the driver
// registers Pdo\Mysql as an internal subclass, and internal inheritance copies the
// whole function struct, so Pdo\Mysql::query() has a handler of its own that a
// guard installed on PDO::query() alone never touches. Both are reachable from
// application code — PDO::connect() is the constructor PHP 8.4 documents — so a
// guard that covers only one of them covers the case that happens to be tested.
$task = oxphp_async(function (): array {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return ['body' => "connect failed: {$errstr} ({$errno})"];
    }
    stream_set_timeout($sock, 8);
    fwrite($sock, "GET /tests/hooksdb/fixture_shared_pdo_connect_hold.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body];
});

oxphp_usleep(200_000);

$t->assertTrue(
    'the holding request created the shared connection before this one woke',
    isset($sharedState['pdo_connect'])
);
$t->assertSame(
    'PDO::connect() returned the driver subclass, which is what makes this case distinct',
    get_class($sharedState['pdo_connect']),
    'Pdo\Mysql'
);

$value = null;
$error = '';
$started = microtime(true);
try {
    $value = $sharedState['pdo_connect']->query('SELECT 42')->fetchColumn();
} catch (\Throwable $e) {
    $error = $e->getMessage();
}
$waited = microtime(true) - $started;

$inner = oxphp_async_await($task);

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
    'connect-hold-done:0'
);

$t->done();
