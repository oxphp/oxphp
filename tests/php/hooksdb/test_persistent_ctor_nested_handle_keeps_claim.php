<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('persistent_ctor_nested_handle_keeps_claim', 'hooksdb');

// A claim on a connection belongs to the request that took it, not to the call
// that happened to take it. PDO's liveness check is the one place that is easy to
// get wrong, because it is the only call that asks about a connection the asking
// request already holds: a request queries, which takes the connection, and then
// builds a second handle on the same pool key, which asks whether that connection
// is alive. A check that gave the claim back at the end of its own work would
// leave the rest of that request unguarded — and it would do it silently, since
// the request's next query takes the connection again.
//
// So the window is the one where nothing takes it back: after the second
// constructor, while the request waits on something that is not the connection.
// The measurement is what another request's constructor meets there. It asks for
// an error mode the holder did not ask for, so it must be given a connection of
// its own; if the holder's claim went missing, that constructor sees a connection
// nobody holds, adopts it, and writes its own error mode onto the handle the
// holder is still using.
$sharedState['ctor_nested_key'] = 'ctor-nested-' . bin2hex(random_bytes(4));
unset($sharedState['ctor_nested_parked'], $sharedState['ctor_nested_released']);

$task = oxphp_async(function (): string {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return "connect failed: {$errstr} ({$errno})";
    }
    stream_set_timeout($sock, 15);
    fwrite($sock, "GET /tests/hooksdb/fixture_persistent_ctor_nested_hold.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return $body;
});

$deadline = microtime(true) + 3.0;
while (!($sharedState['ctor_nested_parked'] ?? false) && microtime(true) < $deadline) {
    oxphp_usleep(20_000);
}
$t->assertTrue(
    'the holding request queried the connection, built a second handle on it, and was waiting',
    $sharedState['ctor_nested_parked'] ?? false
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
            PDO::ATTR_ERRMODE => PDO::ERRMODE_SILENT,
            PDO::ATTR_PERSISTENT => $sharedState['ctor_nested_key'],
        ]
    );
    // Read before the query below, which is on this request's own connection and
    // would race the holder's remaining wait rather than measure it.
    $stillHeld = !($sharedState['ctor_nested_released'] ?? false);
    $mine = $pdo->query('SELECT CONNECTION_ID()')->fetchColumn();
} catch (\Throwable $e) {
    $error = str_replace("\n", ' ', $e->getMessage());
}

// The other end of the window. The flag above stays true after the holder is
// done, so on its own it says only that the holder was there at some point; a
// constructor arriving after the wait meets a connection nobody holds, which is
// shared on purpose and would be read here as this rule breaking.
$t->assertTrue('and had not finished waiting before this request\'s constructor did', $stillHeld);

$body = oxphp_async_await($task);

$t->assertContains('the holding request finished its wait', $body, 'persistent-ctor-nested-done:');
$t->assertSame('this request got its own answer: ' . $error, $error, '');

preg_match(
    '/^persistent-ctor-nested-done: id:(\d+) second-id:(\d+) errmode:(\d+) second:(\d+)$/m',
    $body,
    $m
);

// The premise everything below rests on: the holder's second constructor reached
// the pooled connection rather than opening one of its own. Without it there was
// no liveness check on a claimed connection, and a green run would say nothing.
$t->assertSame(
    'the second handle the holder built is the connection it was already holding',
    $m[2] ?? 'the holder printed no second connection id',
    $m[1] ?? 'the holder printed no connection id'
);

$t->assertNotEqual(
    'this request opened its own connection rather than adopting the one the holder still had',
    (string) $mine,
    $m[1] ?? 'the holder printed no connection id'
);

// Silent is 0 and exception is 2. The holder asked for exceptions on both handles
// and was never told otherwise, so anything else here is this request's own
// options landing on the connection the holder was holding.
$t->assertSame(
    'and the holder kept the error mode it constructed with',
    $m[3] ?? 'the holder printed no error mode',
    (string) PDO::ERRMODE_EXCEPTION
);
$t->assertSame(
    'on the second handle as much as on the first, since both are that one connection',
    $m[4] ?? 'the holder printed no second error mode',
    (string) PDO::ERRMODE_EXCEPTION
);

$t->done();
