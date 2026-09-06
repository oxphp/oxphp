<?php

declare(strict_types=1);

// One side of the burst: the shape an application has when it builds its handle
// per request off a pool key — Laravel and Doctrine both do — so every request
// goes through PDO's pooled lookup and the liveness check in front of it.
//
// The handle is kept rather than dropped at the end of the request. Dropping it
// would run PDO's persistent shutdown on a connection other fibers are using,
// which is a separate defect of its own and would answer a question this fixture
// is not asking.
//
// The line carries the connection id and the wall-clock bounds of the request, so
// the caller can tell whether the pooled connection was reached at all and whether
// these requests really overlapped the ones on the other side.
$started = microtime(true);
try {
    $key = $sharedState['ping_key'] ?? 'ctor-ping-fixed';
    $dsn = 'mysql:host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql')
        . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb');

    $pdo = new PDO($dsn, getenv('DB_USER') ?: 'appuser', getenv('DB_PASS') ?: 'apppass', [
        PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
        PDO::ATTR_PERSISTENT => $key,
    ]);
    $sharedState['ping_handles'][] = $pdo;

    $stmt = $pdo->prepare('SELECT CONNECTION_ID()');
    $stmt->execute();
    $id = $stmt->fetchColumn();

    printf("ping-ctor-done: id:%s %.6f %.6f\n", $id, $started, microtime(true));
} catch (\Throwable $e) {
    printf(
        "ping-ctor-failed:%s %.6f %.6f\n",
        str_replace("\n", ' ', $e->getMessage()),
        $started,
        microtime(true)
    );
}
