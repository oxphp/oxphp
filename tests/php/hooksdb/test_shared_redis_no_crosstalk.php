<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('shared_redis_no_crosstalk', 'hooksdb');

// The same shared-connection case as test_shared_mysql_no_crosstalk on a second,
// independently maintained client. It is worth running both because the two fail
// differently: mysqlnd tracks its own connection state and refuses a command
// issued mid-exchange, so the wire survives, while phpredis has no such check —
// a second fiber's command goes onto the wire and the two replies are read by
// the wrong callers, which is data crossing between requests rather than an
// error.
$key = 'hooksdb:shared:probe';

$task = oxphp_async(function (): array {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return ['body' => "connect failed: {$errstr} ({$errno})"];
    }
    stream_set_timeout($sock, 8);
    fwrite($sock, "GET /tests/hooksdb/fixture_shared_redis_hold.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body];
});

oxphp_usleep(200_000);

$t->assertTrue(
    'the holding request created the shared connection before this one woke',
    isset($sharedState['redis'])
);

$stored = null;
$error = '';
$started = microtime(true);
try {
    $sharedState['redis']->set($key, 'from-the-second-fiber');
    $stored = $sharedState['redis']->get($key);
} catch (\Throwable $e) {
    $error = $e->getMessage();
}
$waited = microtime(true) - $started;

$inner = oxphp_async_await($task);

// The load-bearing assertion. phpredis reads whatever reply is next on the wire,
// so a command written into the middle of the holder's exchange comes back with
// the holder's answer — in this scenario the '+OK' of this fiber's own SET, read
// as the reply to its GET.
$t->assertSame('this fiber read back its own value', $stored, 'from-the-second-fiber');
$t->assertSame('the shared connection reported no protocol failure', $error, '');
$t->assertNotContains(
    'the holding request did not get this fiber\'s reply in place of its own',
    $inner['body'],
    "'+OK'"
);
// Weaker than it looks and kept as a bound rather than as proof: a prober whose
// command did land mid-exchange also spends this long, because the server is busy
// with the holder's blocking pop either way. It rules out the two never having
// overlapped, which is what would make the assertions above vacuous.
$t->assertGreaterThan(
    'the commands did not complete before the fiber holding the connection finished',
    $waited,
    0.5
);

$t->done();
