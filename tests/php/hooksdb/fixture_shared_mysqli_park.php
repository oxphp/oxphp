<?php

declare(strict_types=1);

// The same park as fixture_raw_socket_park.php, reached the way an application
// reaches it: a client built once and shared by every request the worker serves,
// waiting for a query's answer. Its own key, because the request that fires this
// one closes the connection and no other test's connection may go with it.
try {
    if (!isset($sharedState['mysqli_doomed'])) {
        $mysqli = new mysqli(
            getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql',
            getenv('DB_USER') ?: 'appuser',
            getenv('DB_PASS') ?: 'apppass',
            getenv('DB_NAME') ?: 'appdb'
        );
        $sharedState['mysqli_doomed'] = $mysqli;
    }

    // Between the connect and the query, so the closing request can tell that the
    // connection exists and this one is about to use it.
    $sharedState['mysqli_doomed_parked'] = true;

    // Long enough that the answer cannot be what ends this request: the other
    // request waits out its own bound for the connection first, and only then
    // closes it.
    $result = $sharedState['mysqli_doomed']->query('SELECT SLEEP(9)');
    echo 'mysqli-park-done:' . var_export($result !== false, true);
} catch (\Throwable $e) {
    echo 'mysqli-park-failed:' . str_replace("\n", ' ', $e->getMessage());
}
