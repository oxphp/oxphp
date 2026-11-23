<?php

declare(strict_types=1);

// A plain (not persistent) constructor to a data source that accepts the
// connection and never answers: the test holds a listening socket it never
// accepts on, so mysqlnd connects and then waits for a greeting that does not
// come, parked, until the test closes the listener. Both edges of the
// constructor are published for the test to read at the moment it measures.

// The handlers a worker runs with are whatever the last request to set them
// installed — the outer test's, here, which throws on every error.
set_error_handler(null);
set_exception_handler(null);

$sharedState['error_mode_entered'] = true;
try {
    new PDO((string) ($sharedState['error_mode_dsn'] ?? ''), 'nobody', 'nothing', [
        PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
    ]);
    echo "black-hole-built\n";
} catch (\Throwable $e) {
    echo 'black-hole-failed:' . get_class($e) . ':' . str_replace("\n", ' ', $e->getMessage()) . "\n";
} finally {
    $sharedState['error_mode_entered'] = false;
}
