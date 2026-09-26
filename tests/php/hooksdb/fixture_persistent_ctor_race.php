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
require_once __DIR__ . '/pdo_stringable_dsn.php';

try {
    $key = $sharedState['ctor_race_key'] ?? 'ctor-race-fixed';
    $dsn = 'mysql:host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql')
        . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb');

    // How this request names that data source, taken in arrival order from a list
    // the test fills; spelled out in full unless the test says otherwise. Each of
    // the others names the same pool entry as the literal one here: an alias from
    // php.ini, a file the DSN is read from, an object PDO converts to a string.
    $via = isset($sharedState['ctor_race_vias'])
        ? (array_shift($sharedState['ctor_race_vias']) ?? 'literal')
        : 'literal';
    $name = match ($via) {
        'alias' => 'hooksdb',
        'uri' => 'uri:file://' . $sharedState['ctor_race_uri_file'],
        'split-a' => $dsn . ';',
        'split-b' => $dsn . ';:oxgate_a',
        // The same database once more, named past a host the driver then overrides:
        // a later key wins. What the test wants from this spelling is its text, not
        // its target — see the black-hole test.
        'collide' => 'mysql:host=127.0.0.1;port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb')
            . ';host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql'),
        default => $dsn,
    };
    // PDO joins DSN, user and password into its pool key with colons and escapes
    // nothing, so these two name one pool entry with different DSNs:
    // "<dsn>;" + "oxgate_a" + "oxgate_b:pw" and "<dsn>;:oxgate_a" + "oxgate_b" + "pw".
    // The segment after the last semicolon has no `=`, so the driver ignores it and
    // both reach the same server. The test creates both accounts.
    [$user, $pass] = match ($via) {
        'split-a' => ['oxgate_a', 'oxgate_b:pw'],
        'split-b' => ['oxgate_b', 'pw'],
        default => [getenv('DB_USER') ?: 'appuser', getenv('DB_PASS') ?: 'apppass'],
    };

    // Timed and reported, so the caller can tell whether these constructors
    // actually overlapped: four of them run one after another would name one
    // connection honestly, having raced over nothing.
    $started = microtime(true);
    $options = [
        PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
        PDO::ATTR_PERSISTENT => $key,
    ];
    // PHP 8.5 deprecates reading a DSN from a URI; this names that spelling on
    // purpose, so the notice is not the failure being looked for.
    if ($via === 'uri') {
        set_error_handler(static fn (): bool => true, E_DEPRECATED);
    }
    try {
        $pdo = $via === 'object'
            ? oxphp_test_pdo_from_stringable($dsn, $user, $pass, $options)
            : new PDO($name, $user, $pass, $options);
    } finally {
        if ($via === 'uri') {
            restore_error_handler();
        }
    }
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

    printf("ctor-race-via:%s\nctor-race-done:%s %.6f %.6f\n", $via, $row['id'], $started, $built);
} catch (\Throwable $e) {
    echo 'ctor-race-failed:' . str_replace("\n", ' ', $e->getMessage());
}
