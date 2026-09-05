<?php

declare(strict_types=1);

// One of several concurrent requests that all build a persistent PDO for the same
// DSN at the same moment — the shape every worker-mode application has on a fresh
// worker, where `static $pdo ??= new PDO(...)` is reached by whichever requests
// arrive first, together.
//
// PDO looks the pooled connection up before it connects and registers it only
// afterwards, so two constructors overlapping both miss the pool, both connect,
// and the second registration replaces the first — dropping a connection the
// first request is at that moment reading its own reply on.
//
// The key is handed in by the test so each run races from an empty pool: with a
// fixed key only the very first run of a worker would race at all, and every run
// after it would pass without exercising anything.
try {
    $key = $sharedState['ctor_race_key'] ?? 'ctor-race-fixed';
    $dsn = 'mysql:host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql')
        . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb');

    // Timed and reported, so the caller can tell whether these constructors
    // actually overlapped: four of them run one after another would name one
    // connection honestly, having raced over nothing.
    $started = microtime(true);
    $pdo = new PDO($dsn, getenv('DB_USER') ?: 'appuser', getenv('DB_PASS') ?: 'apppass', [
        PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
        PDO::ATTR_PERSISTENT => $key,
    ]);
    $built = microtime(true);

    // Held open across the other requests' constructors by the sleeping query
    // itself. Written as prepare/execute because that is the shape a data-access
    // layer has, not because the prepare adds a step on the wire: pdo_mysql
    // emulates prepared statements unless told otherwise, so nothing is sent
    // until execute(), and the exchange this parks in is that one. The waits are
    // cumulative — each request holds the connection to its own end — so the last
    // of four waits out the three before it: 0.6s of sleeping against the two
    // seconds this image bounds such a wait at, leaving about 0.46s per request
    // for everything else. Comfortable on an idle runner, and the first thing to
    // look at if this profile ever goes intermittent under load.
    $stmt = $pdo->prepare('SELECT CONNECTION_ID() AS id, SLEEP(0.2) AS slept');
    $stmt->execute();
    $row = $stmt->fetch(PDO::FETCH_ASSOC);

    printf("ctor-race-done:%s %.6f %.6f\n", $row['id'], $started, $built);
} catch (\Throwable $e) {
    echo 'ctor-race-failed:' . str_replace("\n", ' ', $e->getMessage());
}
