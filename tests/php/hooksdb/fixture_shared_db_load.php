<?php

declare(strict_types=1);

// One short query on a connection the worker shares between every request — the
// shape a real application has, and the one that puts two complete exchanges back
// to back rather than parking a long one and probing it late.
//
// Under a key of its own, so the tests that assert their holder created the shared
// handle cannot be satisfied by a connection this fixture left behind.
//
// The body is a single line beginning with the outcome, so a load run can count
// served against failed requests by grepping, and it carries the wall-clock bounds
// of the query so a caller running several of these can tell whether they actually
// overlapped.
try {
    $sharedState['pdo_concurrent'] ??= new PDO(
        'mysql:host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql')
            . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb'),
        getenv('DB_USER') ?: 'appuser',
        getenv('DB_PASS') ?: 'apppass',
        [PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION]
    );

    $started = microtime(true);
    $value = $sharedState['pdo_concurrent']->query('SELECT 7')->fetchColumn();
    $ended = microtime(true);

    printf(
        "%s %.6f %.6f\n",
        $value === 7 ? 'load-ok' : 'load-wrong:' . var_export($value, true),
        $started,
        $ended
    );
} catch (\Throwable $e) {
    echo 'load-failed:' . str_replace("\n", ' ', $e->getMessage()) . "\n";
}
