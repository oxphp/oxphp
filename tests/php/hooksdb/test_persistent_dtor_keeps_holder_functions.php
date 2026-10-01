<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('persistent_dtor_keeps_holder_functions', 'hooksdb');

// The same destructor as the transaction case, through its other driver call:
// PDO runs the driver's end-of-request cleanup whenever an object on a
// persistent connection is destroyed, however many other objects still share
// that connection. pdo_sqlite's cleanup unregisters every SQL function on it, so
// a request dropping a handle it adopted takes the functions away from the
// request that is using the connection — which then fails with "no such
// function" on a statement it had every reason to expect to work.
$sharedState['dtor_sqlite_key'] = 'dtor-sqlite-' . bin2hex(random_bytes(4));
unset(
    $sharedState['dtor_sqlite_parked'],
    $sharedState['dtor_sqlite_dropped'],
    $sharedState['dtor_sqlite_released']
);

// First the case nobody is holding, which must stay as PDO has it: a handle
// dropped on a connection no request has made a call on yet cleans it up, so the
// next handle on that pool entry does not inherit the function.
$idleOpts = [
    PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
    PDO::ATTR_PERSISTENT => 'dtor-sqlite-idle-' . bin2hex(random_bytes(4)),
];
$idle = PDO::connect('sqlite::memory:', null, null, $idleOpts);
$idle->createFunction('ox_dtor_idle_f', fn ($x) => $x, 1);
unset($idle);
$idleAgain = PDO::connect('sqlite::memory:', null, null, $idleOpts);
$idleAfter = '';
try {
    $idleAfter = 'still-registered:' . $idleAgain->query('SELECT ox_dtor_idle_f(1)')->fetchColumn();
} catch (\PDOException $e) {
    $idleAfter = str_contains($e->getMessage(), 'no such function') ? 'unregistered' : $e->getMessage();
}
$t->assertSame(
    'a handle dropped on a connection nobody holds still unregisters its functions',
    $idleAfter,
    'unregistered'
);
unset($idleAgain);

$task = oxphp_async(function (): array {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return ['body' => "connect failed: {$errstr} ({$errno})"];
    }
    stream_set_timeout($sock, 15);
    fwrite($sock, "GET /tests/hooksdb/fixture_persistent_dtor_sqlite_hold.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body];
});

$deadline = microtime(true) + 3.0;
while (!($sharedState['dtor_sqlite_parked'] ?? false) && microtime(true) < $deadline) {
    oxphp_usleep(20_000);
}
$t->assertTrue(
    'the holding request registered its function before this one woke',
    $sharedState['dtor_sqlite_parked'] ?? false
);

$error = '';
$stillHeld = false;
$keep = null;
try {
    $opts = [
        PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
        PDO::ATTR_PERSISTENT => $sharedState['dtor_sqlite_key'],
    ];
    $pdo = new PDO('sqlite::memory:', null, null, $opts);
    // Kept to ask afterwards which connection the two were given; asking now
    // would wait for the holder.
    $keep = new PDO('sqlite::memory:', null, null, $opts);

    unset($pdo);

    $stillHeld = !($sharedState['dtor_sqlite_released'] ?? false);
} catch (\Throwable $e) {
    $error = str_replace("\n", ' ', $e->getMessage());
}
$sharedState['dtor_sqlite_dropped'] = true;

$t->assertSame('this request built its handles: ' . $error, $error, '');
$t->assertTrue('and dropped one while the holder was still using the connection', $stillHeld);

$body = oxphp_async_await($task)['body'];

$t->assertContains(
    'the holding request could still call the function it registered',
    $body,
    'persistent-dtor-sqlite-done: before:42 '
);
$t->assertContains(
    'and dropping its own handle unregistered it, as PDO always has',
    $body,
    ' after:unregistered'
);

// The premise: the handle dropped was on the holder's connection. An in-memory
// database is private to the connection that opened it, so the holder's table is
// visible exactly where the holder's connection is.
$seen = $keep !== null
    ? (string) $keep->query("SELECT count(*) FROM sqlite_master WHERE name = 'ox_dtor_marker'")->fetchColumn()
    : '';
$t->assertSame('and the handle dropped was one on the holder\'s connection', $seen, '1');

$t->done();
