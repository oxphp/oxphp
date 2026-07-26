<?php

declare(strict_types=1);

// The holder for the persistent-connection case: one connection reached through
// TWO PDO objects. PDO::ATTR_PERSISTENT keeps the driver's connection handle in
// the persistent pool, so every object built from the same DSN gets that one
// connection — which means the PHP object cannot be what identifies a connection.
//
// Both objects are built here, before either is used, so that neither is
// constructed while the other is mid-exchange (a constructor reaching a pooled
// connection is its own separate problem, not this one). The two CONNECTION_ID()
// values are echoed so the test can confirm the premise instead of assuming it:
// if the pool ever stopped sharing, this test would otherwise pass vacuously.
//
// Not a TestCase — the body is read by the request that started this one, so a
// failure has to arrive as text rather than as an exception page.
try {
    if (!isset($sharedState['pp1'])) {
        $dsn = 'mysql:host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql')
            . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb');
        $opts = [
            PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
            PDO::ATTR_PERSISTENT => true,
        ];
        $sharedState['pp1'] = new PDO($dsn, getenv('DB_USER') ?: 'appuser', getenv('DB_PASS') ?: 'apppass', $opts);
        $sharedState['pp2'] = new PDO($dsn, getenv('DB_USER') ?: 'appuser', getenv('DB_PASS') ?: 'apppass', $opts);
    }

    $id1 = $sharedState['pp1']->query('SELECT CONNECTION_ID()')->fetchColumn();
    $id2 = $sharedState['pp2']->query('SELECT CONNECTION_ID()')->fetchColumn();

    $slept = $sharedState['pp1']->query('SELECT SLEEP(1)')->fetchColumn();
    echo 'persistent-hold-done:' . var_export($slept, true)
        . ' same-connection:' . var_export($id1 === $id2, true);
} catch (\Throwable $e) {
    echo 'persistent-hold-failed:' . str_replace("\n", ' ', $e->getMessage());
}
