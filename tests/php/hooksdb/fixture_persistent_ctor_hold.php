<?php

declare(strict_types=1);

// The holder for the case where a persistent connection is looked up while
// another fiber is mid-exchange on it: this request builds the connection, takes
// it, and parks inside a query on it, so the request that fires this one reaches
// PDO's constructor while the connection is busy.
//
// A first, short query before the flag: it is what makes this request the holder
// of the connection rather than merely its creator, so the constructor on the
// other side meets a connection that is taken and not just one that exists.
try {
    $key = $sharedState['ctor_hold_key'] ?? 'ctor-hold-fixed';
    $dsn = 'mysql:host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql')
        . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb');

    $pdo = new PDO($dsn, getenv('DB_USER') ?: 'appuser', getenv('DB_PASS') ?: 'apppass', [
        PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
        PDO::ATTR_PERSISTENT => $key,
    ]);

    $id = $pdo->query('SELECT CONNECTION_ID()')->fetchColumn();
    $sharedState['ctor_hold_parked'] = true;

    $slept = $pdo->query('SELECT SLEEP(1)')->fetchColumn();

    // Both ends of the window are published, not just its start: the first flag
    // alone stays true after this request is over, so a test reading it can only
    // say the holder took the connection at some point, never that it still had
    // it while the constructor being measured ran. A constructor arriving after
    // this line meets a connection nobody is using, which answers the opposite
    // question and looks like either verdict depending on the test.
    $sharedState['ctor_hold_released'] = true;

    // The error mode is reported back because it is the plainest thing a
    // constructor on the other side can change on a connection it adopts: PDO
    // writes it onto the pooled handle from its own options, defaults included,
    // and nothing tells the request holding that handle it happened.
    echo 'persistent-ctor-hold-done:' . var_export($slept, true) . ' id:' . $id
        . ' errmode:' . $pdo->getAttribute(PDO::ATTR_ERRMODE);
} catch (\Throwable $e) {
    echo 'persistent-ctor-hold-failed:' . str_replace("\n", ' ', $e->getMessage());
}
