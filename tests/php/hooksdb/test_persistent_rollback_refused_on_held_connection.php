<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('persistent_rollback_refused_on_held_connection', 'hooksdb');

// PDO::rollBack() reaches the same driver method PDO's destructor does. A request
// that adopted a pooled connection another request is in the middle of a
// transaction on gets there only once its wait for the connection has given up —
// and the transaction it would then roll back is the holder's, not its own.
//
// The wait is bounded by the startup value of default_socket_timeout, which this
// profile's image sets to two seconds (tests/fixtures/hooks_db/Dockerfile); an
// ini_set() here would not move it. The holder stays parked for up to five, so
// this request gives up while the holder is still between its statements, which
// is where a ROLLBACK would reach the server rather than be refused by the client
// for arriving mid-exchange.
$sharedState['dtor_txn_key'] = 'rollback-held-' . bin2hex(random_bytes(4));
unset(
    $sharedState['dtor_txn_parked'],
    $sharedState['dtor_txn_dropped'],
    $sharedState['dtor_txn_released']
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
    fwrite($sock, "GET /tests/hooksdb/fixture_persistent_dtor_txn_hold.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body];
});

$deadline = microtime(true) + 3.0;
while (!($sharedState['dtor_txn_parked'] ?? false) && microtime(true) < $deadline) {
    oxphp_usleep(20_000);
}
$t->assertTrue(
    'the holding request opened its transaction before this one woke',
    $sharedState['dtor_txn_parked'] ?? false
);

$error = '';
$result = null;
$waited = 0.0;
$stillHeld = false;
$keep = null;
try {
    $opts = [
        PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
        PDO::ATTR_PERSISTENT => $sharedState['dtor_txn_key'],
    ];
    $keep = new PDO($dsn, $user, $pass, $opts);

    $start = microtime(true);
    $result = $keep->rollBack();
    $waited = microtime(true) - $start;

    // Read before anything that would wait for the holder, which always finds it
    // done.
    $stillHeld = !($sharedState['dtor_txn_released'] ?? false);
} catch (\Throwable $e) {
    $error = str_replace("\n", ' ', $e->getMessage());
}
$sharedState['dtor_txn_dropped'] = true;

$t->assertSame('the call returned rather than threw: ' . $error, $error, '');
$t->assertTrue('this request gave up waiting for the connection first', $waited >= 1.5);
$t->assertTrue('while the holder was still inside its transaction', $stillHeld);
$t->assertFalse('and rollBack() refused the holder\'s transaction', $result);

$body = oxphp_async_await($task)['body'];

$t->assertContains(
    'the holding request found its transaction still open and committed it',
    $body,
    'persistent-dtor-txn-done: in_txn:true rows:1'
);

preg_match('/ id:(\d+)$/', $body, $m);
$mine = $keep !== null ? (string) $keep->query('SELECT CONNECTION_ID()')->fetchColumn() : '';
$t->assertSame(
    'and the handle rolled back was one on the holder\'s connection',
    $mine,
    $m[1] ?? 'the holder printed no connection id'
);
unset($keep);

$t->done();
