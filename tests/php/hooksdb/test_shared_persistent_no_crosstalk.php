<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('shared_persistent_no_crosstalk', 'hooksdb');

// One persistent connection, two PDO objects, two request fibers. The holder parks
// mid-exchange on one object and this request uses the other, so the two calls meet
// on the same connection while naming different objects — which is the case a claim
// kept per PHP object cannot see at all.
$task = oxphp_async(function (): array {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return ['body' => "connect failed: {$errstr} ({$errno})"];
    }
    stream_set_timeout($sock, 8);
    fwrite($sock, "GET /tests/hooksdb/fixture_shared_persistent_hold.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body];
});

oxphp_usleep(200_000);

$t->assertTrue(
    'the holding request created both handles before this one woke',
    isset($sharedState['pp1']) && isset($sharedState['pp2'])
);

$value = null;
$error = '';
$started = microtime(true);
try {
    $value = $sharedState['pp2']->query('SELECT 42')->fetchColumn();
} catch (\Throwable $e) {
    $error = $e->getMessage();
}
$waited = microtime(true) - $started;

$inner = oxphp_async_await($task);

// The premise first: without it the rest of the test proves nothing, because two
// objects on two connections never had a conflict to avoid.
$t->assertContains(
    'both PDO objects really do share one server connection',
    $inner['body'],
    'same-connection:true'
);
$t->assertSame('this fiber got the answer to its own query', $value, 42);
$t->assertSame('the shared connection reported no client-side protocol failure', $error, '');
$t->assertGreaterThan(
    'the query waited for the fiber holding the connection through the other object',
    $waited,
    0.5
);
$t->assertContains(
    'the holding request finished with the answer to its own query',
    $inner['body'],
    'persistent-hold-done:0'
);

$t->done();
