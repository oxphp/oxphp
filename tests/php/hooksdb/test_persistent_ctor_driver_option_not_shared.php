<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('persistent_ctor_driver_option_not_shared', 'hooksdb');

// The other edge of the same rule, and the one an application is most likely to
// stand on: a pooled connection in use is shared only with a constructor whose
// options PDO stores on the handle and nothing more. An option the driver has to
// be told about is refused whatever its value, because being told is a command,
// and a command here lands inside the holder's exchange — where a write the claim
// refuses reads to the client as a server that has gone away.
//
// PDO::ATTR_AUTOCOMMIT is the sharpest way to say that. PDO writes `auto_commit`
// onto the handle from the options with its own default of 1 when the key is
// absent, so the holder — which passes no such key — is at 1 already, and asking
// for `true` here asks for the value that is there. Sharing is still refused,
// because what PDO does with that key next is call the driver's set_attribute
// with it. Same value, different answer from PDO::ATTR_ERRMODE at the same value:
// the difference is where the write goes, not what it is.
$sharedState['ctor_hold_key'] = 'ctor-driver-' . bin2hex(random_bytes(4));
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

// Waited for rather than slept past: a constructor arriving after the holder is
// done meets a connection nobody holds and proves nothing either way.
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
            PDO::ATTR_AUTOCOMMIT => true,
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

$t->assertNotEqual(
    'this request opened its own connection rather than adopting the busy one',
    (string) $mine,
    $m[1] ?? 'the holder printed no connection id'
);

// And nothing reached the holder on the way to that answer.
$t->assertSame(
    'the holder kept the error mode it constructed with',
    $m[2] ?? 'the holder printed no error mode',
    (string) PDO::ERRMODE_EXCEPTION
);

$t->done();
