<?php

declare(strict_types=1);

// Served in its own request fiber, and the first request to touch the shared
// MySQL connection: it creates the handle the worker keeps for its whole life
// ($sharedState comes from the worker entry, which `include` puts in scope) and
// then holds a query open on it for a second. Under the socket hooks that wait
// parks this fiber mid-exchange, which is exactly the window the test's own
// request has to use the same connection in.
//
// Not a TestCase — the body is read by the request that started this one, so a
// failure has to arrive as text rather than as an exception page.
try {
    $sharedState['pdo'] ??= new PDO(
        'mysql:host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql')
            . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb'),
        getenv('DB_USER') ?: 'appuser',
        getenv('DB_PASS') ?: 'apppass',
        [PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION]
    );

    $slept = $sharedState['pdo']->query('SELECT SLEEP(1)')->fetchColumn();
    echo 'hold-done:' . var_export($slept, true);
} catch (\Throwable $e) {
    echo 'hold-failed:' . $e->getMessage();
}
