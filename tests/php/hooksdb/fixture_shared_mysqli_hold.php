<?php

declare(strict_types=1);

// The mysqli half of the shared-connection case, and the first request to touch
// the shared link: it creates the connection the worker keeps for its whole life
// ($sharedState comes from the worker entry, which `include` puts in scope) and
// then holds a one-second query open on it.
//
// Written in the procedural form throughout, because that is the form WordPress
// uses — the connection arrives as the call's first argument rather than as
// $this, and the two are separate entry points.
//
// Not a TestCase — the body is read by the request that started this one, so a
// failure has to arrive as text rather than as an exception page.
try {
    if (!isset($sharedState['mysqli'])) {
        $sharedState['mysqli'] = mysqli_connect(
            getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql',
            getenv('DB_USER') ?: 'appuser',
            getenv('DB_PASS') ?: 'apppass',
            getenv('DB_NAME') ?: 'appdb'
        );
    }

    $result = mysqli_query($sharedState['mysqli'], 'SELECT SLEEP(1)');
    $row = mysqli_fetch_row($result);
    echo 'mysqli-hold-done:' . var_export($row[0], true);
} catch (\Throwable $e) {
    echo 'mysqli-hold-failed:' . $e->getMessage();
}
