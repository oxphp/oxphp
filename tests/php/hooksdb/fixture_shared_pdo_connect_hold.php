<?php

declare(strict_types=1);

// The same holder as fixture_shared_db_hold.php, but opening the connection with
// PDO::connect() — the constructor PHP 8.4 added, which returns the driver's own
// subclass (Pdo\Mysql) rather than PDO. An internal subclass carries its own copy
// of every method it inherits, so a guard installed on PDO alone does not apply to
// a connection opened this way.
//
// Not a TestCase — the body is read by the request that started this one, so a
// failure has to arrive as text rather than as an exception page.
try {
    $sharedState['pdo_connect'] ??= PDO::connect(
        'mysql:host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql')
            . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb'),
        getenv('DB_USER') ?: 'appuser',
        getenv('DB_PASS') ?: 'apppass',
        [PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION]
    );

    $slept = $sharedState['pdo_connect']->query('SELECT SLEEP(1)')->fetchColumn();
    echo 'connect-hold-done:' . var_export($slept, true)
        . ' cls=' . get_class($sharedState['pdo_connect']);
} catch (\Throwable $e) {
    echo 'connect-hold-failed:' . str_replace("\n", ' ', $e->getMessage());
}
