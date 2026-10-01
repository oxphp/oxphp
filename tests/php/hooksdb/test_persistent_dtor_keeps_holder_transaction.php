<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('persistent_dtor_keeps_holder_transaction', 'hooksdb');

// Destroying a PDO object is not only a matter of dropping a reference. PDO's
// destructor rolls back the transaction the connection reports open and then
// runs the driver's end-of-request cleanup, and it does both before it looks at
// how many objects still share the pooled connection. A request that adopted a
// connection another request is in the middle of a transaction on therefore ends
// that transaction by letting go of its own handle — without ever having begun
// one.
//
// Its own key per run, so this request meets the connection the holder built
// rather than one an earlier run left in the pool.
$sharedState['dtor_txn_key'] = 'dtor-txn-' . bin2hex(random_bytes(4));
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
$stillHeld = false;
$keep = null;
try {
    // The same options the holder used, so the constructor shares the busy
    // connection instead of opening one of its own. Two handles: the one dropped
    // below, and one kept to ask afterwards which connection they were given —
    // asking now would wait for the holder to finish.
    $opts = [
        PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
        PDO::ATTR_PERSISTENT => $sharedState['dtor_txn_key'],
    ];
    $pdo = new PDO($dsn, $user, $pass, $opts);
    $keep = new PDO($dsn, $user, $pass, $opts);

    unset($pdo);

    // Read before anything that would wait for the holder, which always finds it
    // done.
    $stillHeld = !($sharedState['dtor_txn_released'] ?? false);
} catch (\Throwable $e) {
    $error = str_replace("\n", ' ', $e->getMessage());
}
$sharedState['dtor_txn_dropped'] = true;

$t->assertSame('this request built its handles: ' . $error, $error, '');
$t->assertTrue('and dropped one while the holder was still inside its transaction', $stillHeld);

$body = oxphp_async_await($task)['body'];

// The holder's transaction is still its own: the connection still reports it
// open, and committing it keeps the row.
$t->assertContains(
    'the holding request found its transaction still open and committed it',
    $body,
    'persistent-dtor-txn-done: in_txn:true rows:1'
);

// And the premise: the dropped handle was on the holder's connection, not on one
// of its own. Different ids mean the constructor opened a second connection, and
// then the handle dropped above never touched the holder's.
preg_match('/ id:(\d+)$/', $body, $m);
$mine = $keep !== null ? (string) $keep->query('SELECT CONNECTION_ID()')->fetchColumn() : '';
$t->assertSame(
    'and the handle dropped was one on the holder\'s connection',
    $mine,
    $m[1] ?? 'the holder printed no connection id'
);
unset($keep);

// A handle dropped by the request that owns the transaction still rolls it back,
// which is what PDO has always done: only a handle whose request does not hold
// the connection is kept away from it.
$ownKey = 'dtor-txn-own-' . bin2hex(random_bytes(4));
$ownOpts = [PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION, PDO::ATTR_PERSISTENT => $ownKey];
$own = new PDO($dsn, $user, $pass, $ownOpts);
$own->exec('CREATE TEMPORARY TABLE ox_dtor_txn_own (n INT) ENGINE=InnoDB');

// PDO::rollBack() reaches the same driver method the destructor does, and a
// request rolling back its own transaction must still get there.
$own->beginTransaction();
$own->exec('INSERT INTO ox_dtor_txn_own VALUES (1)');
$t->assertTrue('the request holding the connection can roll back its own transaction', $own->rollBack());
$t->assertSame(
    'and the rollback reached the server',
    (string) $own->query('SELECT COUNT(*) FROM ox_dtor_txn_own')->fetchColumn(),
    '0'
);

// The same through a closure, which runs a copy of the method whose handler is
// the closure trampoline rather than the hooked one: it must still be told apart
// from the destructor, or the rollback is skipped and the transaction stays open.
$own->beginTransaction();
$own->exec('INSERT INTO ox_dtor_txn_own VALUES (1)');
$rollBack = $own->rollBack(...);
$t->assertTrue('and through a closure', $rollBack());
$t->assertFalse('which ended the transaction', $own->inTransaction());
$t->assertSame(
    'and the rollback reached the server',
    (string) $own->query('SELECT COUNT(*) FROM ox_dtor_txn_own')->fetchColumn(),
    '0'
);
unset($rollBack);
// Left open by a failure above, the next beginTransaction() would throw and hide
// which assertion it was.
if ($own->inTransaction()) {
    $own->rollBack();
}

$own->beginTransaction();
$own->exec('INSERT INTO ox_dtor_txn_own VALUES (1)');
unset($own);

$again = new PDO($dsn, $user, $pass, $ownOpts);
$t->assertFalse('a handle dropped by the transaction\'s own request rolls it back', $again->inTransaction());
$t->assertSame(
    'and the row it inserted is gone',
    (string) $again->query('SELECT COUNT(*) FROM ox_dtor_txn_own')->fetchColumn(),
    '0'
);

$t->done();
