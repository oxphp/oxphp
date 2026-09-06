<?php

declare(strict_types=1);

// Puts a persistent connection into the pool and leaves it there in the state the
// liveness check is asked about: alive, referenced by an object the worker keeps,
// and claimed by nobody.
//
// The handle goes into $sharedState rather than a local, so it outlives this
// request the way `static $pdo` outlives one in an application — that is what
// keeps the pooled entry's refcount above the pool's own reference. The claim
// does not outlive it: a claim is given up when the request that took it ends, so
// from the moment this request finishes the connection is live and unclaimed,
// which is exactly when PDO's own liveness check reaches the wire.
try {
    $key = $sharedState['ping_key'] ?? 'ctor-ping-fixed';
    $dsn = 'mysql:host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql')
        . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb');

    $sharedState['ping_pdo'] = new PDO($dsn, getenv('DB_USER') ?: 'appuser', getenv('DB_PASS') ?: 'apppass', [
        PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
        PDO::ATTR_PERSISTENT => $key,
    ]);

    $id = $sharedState['ping_pdo']->query('SELECT CONNECTION_ID()')->fetchColumn();

    echo 'ping-create-done: id:' . $id . "\n";
} catch (\Throwable $e) {
    echo 'ping-create-failed:' . str_replace("\n", ' ', $e->getMessage()) . "\n";
}
