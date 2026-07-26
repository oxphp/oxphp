<?php

declare(strict_types=1);

// Holder for the mysqli::connect() case, under a key of its own: its counterpart
// reconnects this very link, so the object must not be one another test depends on
// afterwards.
//
// Object form here, unlike the other mysqli fixture, because connect() is a method
// and the object is what it acts on.
try {
    if (!isset($sharedState['mysqli_conn'])) {
        $sharedState['mysqli_conn'] = new mysqli(
            getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql',
            getenv('DB_USER') ?: 'appuser',
            getenv('DB_PASS') ?: 'apppass',
            getenv('DB_NAME') ?: 'appdb'
        );
    }

    $result = $sharedState['mysqli_conn']->query('SELECT SLEEP(1)');
    $row = $result->fetch_row();
    echo 'mysqli-conn-hold-done:' . var_export($row[0], true);
} catch (\Throwable $e) {
    echo 'mysqli-conn-hold-failed:' . str_replace("\n", ' ', $e->getMessage());
}
