<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('persistent_ctor_driver_without_liveness', 'hooksdb');

// The same question as the tests above — what a constructor does to a pooled
// connection another fiber is using — asked of a driver that has no liveness
// check of its own. pdo_sqlite is one (pdo_dblib is the other in the tree), and
// PDO calls a liveness check only where the driver supplies one, so here it
// hands the pooled connection over without asking anything: the constructor's
// error mode, autocommit flag and remaining options are written straight onto a
// handle whose holder is parked, and the holder's next statement runs under
// whatever it left there.
//
// So the answer cannot be installed only where a driver already asks the
// question. Its own key per run, so this request meets the connection the holder
// built rather than one an earlier run left in the pool.
$sharedState['ctor_sqlite_key'] = 'ctor-sqlite-' . bin2hex(random_bytes(4));
unset($sharedState['ctor_sqlite_parked'], $sharedState['ctor_sqlite_released']);

$task = oxphp_async(function (): array {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return ['body' => "connect failed: {$errstr} ({$errno})"];
    }
    stream_set_timeout($sock, 15);
    fwrite($sock, "GET /tests/hooksdb/fixture_persistent_ctor_sqlite_hold.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body];
});

$deadline = microtime(true) + 3.0;
while (!($sharedState['ctor_sqlite_parked'] ?? false) && microtime(true) < $deadline) {
    oxphp_usleep(20_000);
}
$t->assertTrue(
    'the holding request took the shared connection before this one woke',
    $sharedState['ctor_sqlite_parked'] ?? false
);

$sawMarker = null;
$error = '';
$stillHeld = false;
try {
    // A different error mode is what makes adopting this connection something
    // other than leaving it as it is, and it is the plainest such option: PDO
    // stores it on the handle, so a constructor passing one value onto a
    // connection holding another changes it under whoever is using it.
    $pdo = new PDO('sqlite::memory:', null, null, [
        PDO::ATTR_ERRMODE => PDO::ERRMODE_SILENT,
        PDO::ATTR_PERSISTENT => $sharedState['ctor_sqlite_key'],
    ]);
    // Read before the query below, which waits for the holder to give the
    // connection up and would therefore always find it released.
    $stillHeld = !($sharedState['ctor_sqlite_released'] ?? false);
    $sawMarker = (int) $pdo
        ->query("SELECT count(*) FROM sqlite_master WHERE name = 'ox_holder_marker'")
        ->fetchColumn();
} catch (\Throwable $e) {
    $error = str_replace("\n", ' ', $e->getMessage());
}

$t->assertTrue('and had not let it go before this request\'s constructor finished', $stillHeld);
$t->assertSame('this request built a connection of some kind: ' . $error, $error, '');

// An in-memory database is private to the connection that opened it, so the
// holder's table is visible exactly where the holder's connection is.
$t->assertSame(
    'and got one of its own rather than the one the holder is using',
    (string) $sawMarker,
    '0'
);

$body = oxphp_async_await($task)['body'];

$t->assertContains(
    'the holding request finished what it was parked in',
    $body,
    'persistent-ctor-sqlite-done:'
);
$t->assertContains(
    'and kept the error mode it constructed with',
    $body,
    'errmode:' . PDO::ERRMODE_EXCEPTION
);

$t->done();
