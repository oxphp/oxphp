<?php

declare(strict_types=1);

// The holder for the case where another request drops its own handle on a
// pooled connection this request is in the middle of a transaction on. PDO's
// object destructor rolls back whatever transaction the connection reports open,
// and the connection cannot say whose it is — so a handle that never began one
// ends this request's.
//
// Parked on something other than the database while the transaction is open:
// between two statements is where a transaction spends most of its life, and it
// is where a command from outside reaches the server rather than being refused
// by the client for arriving mid-exchange.
try {
    $key = $sharedState['dtor_txn_key'] ?? 'dtor-txn-fixed';
    $dsn = 'mysql:host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql')
        . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb');

    $pdo = new PDO($dsn, getenv('DB_USER') ?: 'appuser', getenv('DB_PASS') ?: 'apppass', [
        PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
        PDO::ATTR_PERSISTENT => $key,
    ]);

    $id = $pdo->query('SELECT CONNECTION_ID()')->fetchColumn();
    // Private to this connection, so nothing another test does can add to it.
    $pdo->exec('CREATE TEMPORARY TABLE ox_dtor_txn (n INT) ENGINE=InnoDB');
    $pdo->beginTransaction();
    $pdo->exec('INSERT INTO ox_dtor_txn VALUES (1)');
    $sharedState['dtor_txn_parked'] = true;

    // Until the other side says it has dropped its handle, rather than for a fixed
    // time it ought to have taken: a loaded host outrunning a fixed sleep would
    // leave this request committing before anything happened to it.
    $deadline = microtime(true) + 5.0;
    while (!($sharedState['dtor_txn_dropped'] ?? false) && microtime(true) < $deadline) {
        oxphp_usleep(20_000);
    }

    // The other end of the window: past this line the transaction is about to
    // end on its own, and a handle dropped after it proves nothing.
    $sharedState['dtor_txn_released'] = true;

    $inTxn = $pdo->inTransaction();
    $pdo->commit();
    $rows = $pdo->query('SELECT COUNT(*) FROM ox_dtor_txn')->fetchColumn();

    echo 'persistent-dtor-txn-done: in_txn:' . var_export($inTxn, true)
        . ' rows:' . $rows . ' id:' . $id;
} catch (\Throwable $e) {
    echo 'persistent-dtor-txn-failed:' . str_replace("\n", ' ', $e->getMessage());
}
