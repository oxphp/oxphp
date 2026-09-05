<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('persistent_ctor_options_differ', 'hooksdb');

// The other side of sharing a pooled connection with a constructor: PDO does not
// stop at handing the connection over. It writes the error mode and the
// autocommit flag onto the pooled handle from the options it was given — with its
// own defaults when they are absent — and then applies the rest of those options
// to that same handle. On a connection another fiber is mid-exchange on, all of
// that is the other fiber's: its error mode changes under it, and an option the
// driver forwards changes how the rest of its exchange behaves, with nothing
// raised on either side.
//
// So a connection in use is shared only with a constructor that would leave it
// exactly as it is. This one would not — it asks for a different error mode — and
// must therefore get a connection of its own rather than reach into the holder's.
//
// It builds through PDO::connect() rather than `new PDO`, which is the same
// constructor reached the other way, and the one that returns the driver subclass.
$sharedState['ctor_hold_key'] = 'ctor-differ-' . bin2hex(random_bytes(4));
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

// Wait for the holder to have taken the connection rather than sleeping long
// enough that it ought to have — the same reason as in the sharing case: a
// constructor that arrives after the holder is done proves nothing either way.
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
    $pdo = PDO::connect(
        'mysql:host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql')
            . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb'),
        getenv('DB_USER') ?: 'appuser',
        getenv('DB_PASS') ?: 'apppass',
        [
            PDO::ATTR_ERRMODE => PDO::ERRMODE_SILENT,
            PDO::ATTR_PERSISTENT => $sharedState['ctor_hold_key'],
        ]
    );
    // Read before the query below, which is on this request's own connection
    // and would race the holder's remaining sleep rather than measure it.
    $stillHeld = !($sharedState['ctor_hold_released'] ?? false);
    $mine = $pdo->query('SELECT CONNECTION_ID()')->fetchColumn();
} catch (\Throwable $e) {
    $error = str_replace("\n", ' ', $e->getMessage());
}

// The other end of the window the flag above opened. Without it the check
// there stays true once the holder is done, and a constructor arriving after
// that meets a connection nobody holds — which is shared, so the identity
// check below would fail and be read as this rule breaking.
$t->assertTrue('and had not let it go before this request\'s constructor finished', $stillHeld);

$body = oxphp_async_await($task)['body'];

$t->assertContains(
    'the holding request finished the query it was parked in',
    $body,
    'persistent-ctor-hold-done:0'
);
$t->assertSame('this request got its own answer: ' . $error, $error, '');

preg_match('/^persistent-ctor-hold-done:\S+ id:(\d+) errmode:(\d+)$/m', $body, $m);

// The holder asked for exceptions and was never told otherwise. Silent is 0 and
// exception is 2, so this reads back as 2 unless this request's constructor wrote
// its own options onto the connection the holder was using.
$t->assertSame(
    'the holder kept the error mode it constructed with',
    $m[2] ?? 'the holder printed no error mode',
    (string) PDO::ERRMODE_EXCEPTION
);

// And it got its own connection instead of the one in use.
$t->assertNotEqual(
    'this request opened its own connection rather than adopting the busy one',
    (string) $mine,
    $m[1] ?? 'the holder printed no connection id'
);

$t->done();
