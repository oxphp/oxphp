<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('persistent_dtor_gc_rolls_back_orphaned_transaction', 'hooksdb');

// The other side of the same question: a request that holds the connection leaves
// its own handle — its only one — in a garbage cycle mid-transaction, and this
// request runs the collector. The handle is destroyed here, on a fiber that does
// not hold the connection, but it is still the holder's, and dropping it must
// still roll the holder's transaction back. Skipped, the transaction would stay
// open on the pooled connection, with nothing left that could end it, for
// whoever takes the connection next.
//
// This request keeps a handle of its own on the connection throughout, so the
// destroyed handle is never the last one on it.
$sharedState['dtor_gc_key'] = 'dtor-gc-orphan-' . bin2hex(random_bytes(4));
unset(
    $sharedState['dtor_gc_parked'],
    $sharedState['dtor_gc_dropped'],
    $sharedState['dtor_gc_released'],
    $sharedState['dtor_gc_sentinel'],
    $sharedState['dtor_gc_collector']
);

$dsn = 'mysql:host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql')
    . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb');
$user = getenv('DB_USER') ?: 'appuser';
$pass = getenv('DB_PASS') ?: 'apppass';

$task = oxphp_async(function (): array {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return ['body' => "connect failed: {$errstr} ({$errno})"];
    }
    stream_set_timeout($sock, 15);
    fwrite($sock, "GET /tests/hooksdb/fixture_persistent_dtor_gc_hold.php?mode=orphan HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body];
});

$deadline = microtime(true) + 3.0;
while (!($sharedState['dtor_gc_parked'] ?? false) && microtime(true) < $deadline) {
    oxphp_usleep(20_000);
}
$t->assertTrue(
    'the holding request opened its transaction and dropped its handle before this one woke',
    $sharedState['dtor_gc_parked'] ?? false
);

$error = '';
$stillHeld = false;
$keep = null;
try {
    $keep = new PDO($dsn, $user, $pass, [
        PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
        PDO::ATTR_PERSISTENT => $sharedState['dtor_gc_key'],
    ]);

    $sharedState['dtor_gc_collector'] = 'other';
    gc_collect_cycles();
    unset($sharedState['dtor_gc_collector']);

    $stillHeld = !($sharedState['dtor_gc_released'] ?? false);
} catch (\Throwable $e) {
    $error = str_replace("\n", ' ', $e->getMessage());
}
$sharedState['dtor_gc_dropped'] = true;

$t->assertSame('this request built its handle and collected: ' . $error, $error, '');
$t->assertTrue('while the holder still held the connection', $stillHeld);
$t->assertSame(
    'and this request\'s collector is what destroyed the holder\'s handle',
    $sharedState['dtor_gc_sentinel'] ?? 'nobody',
    'other'
);

$body = oxphp_async_await($task)['body'];

$t->assertContains(
    'the holder\'s transaction ended with its handle',
    $body,
    'persistent-dtor-gc-orphan-done: in_txn:false rows:0'
);

preg_match('/ id:(\d+)$/', $body, $m);
$mine = $keep !== null ? (string) $keep->query('SELECT CONNECTION_ID()')->fetchColumn() : '';
$t->assertSame(
    'and the handle collected was one on the connection this request shares',
    $mine,
    $m[1] ?? 'the holder printed no connection id'
);
unset($keep);

$t->done();
