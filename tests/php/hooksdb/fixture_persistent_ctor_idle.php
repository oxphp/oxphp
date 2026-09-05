<?php

declare(strict_types=1);

// A holder of the other kind: this request queries the pooled connection, drops
// its own handle, and then parks on something that is not the connection at all.
//
// A claim is given up at the end of a request, and the object it was taken for
// can be dropped long before that — so from the line below until this request
// ends, the connection is claimed by a fiber that has nothing in flight on it and
// no way to start anything, since the pool is the only thing left referencing it.
// That is the state a constructor arriving here has to be answered in, and the
// answer is not the one a busy connection gets.
try {
    $key = $sharedState['ctor_idle_key'] ?? 'ctor-idle-fixed';
    $dsn = 'mysql:host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql')
        . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb');

    $pdo = new PDO($dsn, getenv('DB_USER') ?: 'appuser', getenv('DB_PASS') ?: 'apppass', [
        PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
        PDO::ATTR_PERSISTENT => $key,
    ]);

    // The query is what makes this request the holder rather than the creator:
    // the claim is taken at the first call through PDO, not by the constructor.
    $id = $pdo->query('SELECT CONNECTION_ID()')->fetchColumn();
    unset($pdo);

    $sharedState['ctor_idle_parked'] = true;
    usleep(500_000);
    $sharedState['ctor_idle_released'] = true;

    echo 'persistent-ctor-idle-done: id:' . $id;
} catch (\Throwable $e) {
    echo 'persistent-ctor-idle-failed:' . str_replace("\n", ' ', $e->getMessage());
}
