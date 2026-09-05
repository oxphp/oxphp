<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('persistent_ctor_under_parked_holder', 'hooksdb');

// The other way a shared persistent connection is lost, and the one an
// application meets long after startup: PDO checks that a pooled connection is
// alive before handing it out, and that check is a ping sent on the connection
// itself. Sent while another fiber is mid-exchange on it, the ping cannot go — so
// PDO reads "dead", drops the pool entry and connects again. The fiber
// mid-exchange keeps its own connection and its reply; what it loses is the
// sharing, because the pool now holds a different connection and every request
// that constructs one from here on gets that one instead.
//
// Its own key per run, so this request meets the connection the holder built
// rather than one left in the pool by an earlier test.
$sharedState['ctor_hold_key'] = 'ctor-hold-' . bin2hex(random_bytes(4));
unset($sharedState['ctor_hold_parked'], $sharedState['ctor_hold_released']);

$task = oxphp_async(function (): array {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return ['body' => "connect failed: {$errstr} ({$errno})"];
    }
    stream_set_timeout($sock, 15);
    fwrite($sock, "GET /tests/hooksdb/fixture_persistent_ctor_hold.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body];
});

// Wait for the holder to have taken the connection, rather than sleeping long
// enough that it ought to have: a fixed sleep a loaded host outruns leaves this
// request constructing against a connection nobody holds, which proves nothing.
$deadline = microtime(true) + 3.0;
while (!($sharedState['ctor_hold_parked'] ?? false) && microtime(true) < $deadline) {
    oxphp_usleep(20_000);
}
$t->assertTrue(
    'the holding request took the shared connection before this one woke',
    $sharedState['ctor_hold_parked'] ?? false
);

$mine = null;
$error = '';
$stillHeld = false;
try {
    $pdo = new PDO(
        'mysql:host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql')
            . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb'),
        getenv('DB_USER') ?: 'appuser',
        getenv('DB_PASS') ?: 'apppass',
        [
            PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
            PDO::ATTR_PERSISTENT => $sharedState['ctor_hold_key'],
        ]
    );
    // Read here rather than after the query below, which waits for the holder to
    // give the connection up and would therefore always find it released.
    $stillHeld = !($sharedState['ctor_hold_released'] ?? false);
    $mine = $pdo->query('SELECT CONNECTION_ID()')->fetchColumn();
} catch (\Throwable $e) {
    $error = str_replace("\n", ' ', $e->getMessage());
}

// The other end of the window the flag above opened: the two together are what
// say the constructor ran while the holder had the connection, rather than after
// it was done — which is a connection nobody holds, and proves nothing here.
$t->assertTrue('and had not let it go before this request\'s constructor finished', $stillHeld);

$body = oxphp_async_await($task)['body'];

// The holder first: it asked nothing of this request and must not have been
// ended by it.
$t->assertContains(
    'the holding request finished the query it was parked in',
    $body,
    'persistent-ctor-hold-done:0'
);
$t->assertSame('this request got its own answer: ' . $error, $error, '');

// And the mechanism: the constructor took the connection that was already there
// instead of declaring it dead. Different ids mean the pooled entry was thrown
// away and rebuilt, leaving the worker holding two connections where the
// application asked for one — and a pool entry rebuilt that way is what the next
// pair of constructors races to replace, which is where a connection is freed
// under the request using it.
preg_match('/^persistent-ctor-hold-done:\S+ id:(\d+) errmode:(\d+)$/m', $body, $m);
$t->assertSame(
    'and it reused the busy connection rather than dropping it and building another',
    (string) $mine,
    $m[1] ?? 'the holder printed no connection id'
);

// Sharing is only safe while the constructor doing it leaves the connection as it
// found it, which is what this one asked for: the same options the holder used.
$t->assertSame(
    'and left the holder the error mode it constructed with',
    $m[2] ?? 'the holder printed no error mode',
    (string) PDO::ERRMODE_EXCEPTION
);

$t->done();
