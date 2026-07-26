<?php

declare(strict_types=1);

// Holder for the attribute-filter test, under a key of its own so that test's
// premise cannot be satisfied by a connection some other test left behind. Same
// shape as the other holders: create the shared handle, then park mid-exchange on
// it for a second.
try {
    $sharedState['pdo_attr'] ??= new PDO(
        'mysql:host=' . (getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql')
            . ';port=3306;dbname=' . (getenv('DB_NAME') ?: 'appdb'),
        getenv('DB_USER') ?: 'appuser',
        getenv('DB_PASS') ?: 'apppass',
        [PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION]
    );

    $slept = $sharedState['pdo_attr']->query('SELECT SLEEP(1)')->fetchColumn();
    echo 'attr-hold-done:' . var_export($slept, true);
} catch (\Throwable $e) {
    echo 'attr-hold-failed:' . str_replace("\n", ' ', $e->getMessage());
}
