<?php

declare(strict_types=1);

// fixture_shared_mysqli_park.php with the wait moved into a destructor that runs
// while an exception unwinds: the frame holding an unbuffered result is left by
// a throw, freeing the result reads off the rows still to come, and that read is
// parked on the shared connection when the firing request closes it. Its own
// key, for the same reason as there.
//
// The firing request's TestCase handler is the worker's handler too, and would
// turn any warning raised here into an exception thrown in the middle of it.
set_error_handler(null);
set_exception_handler(null);

// What this request reports on the way into the fatal that ends it, kept where
// the firing request can read it back.
ini_set('log_errors', '1');
ini_set('error_log', '/tmp/oxphp-unwind-reader-' . preg_replace('/[^a-z0-9]/', '', (string) ($_GET['key'] ?? '')) . '.log');

if (!isset($sharedState['mysqli_unwind_doomed'])) {
    $sharedState['mysqli_unwind_doomed'] = new mysqli(
        getenv('DB_MYSQL_HOST') ?: 'hooksdb-mysql',
        getenv('DB_USER') ?: 'appuser',
        getenv('DB_PASS') ?: 'apppass',
        getenv('DB_NAME') ?: 'appdb'
    );
}

$read = static function (mysqli $mysqli) use (&$sharedState): void {
    // The first row at once, the second only after nine seconds: long enough
    // that its arrival cannot be what ends this request, as there. The first is
    // larger than the server's network buffer, which would otherwise hold it
    // back until the second is ready.
    $result = $mysqli->query(
        "SELECT n, IF(n = 1, REPEAT('x', 40000), SLEEP(9)) FROM (SELECT 1 AS n UNION ALL SELECT 2) t",
        MYSQLI_USE_RESULT
    );
    $result->fetch_row();
    $sharedState['mysqli_unwind_doomed_parked'] = true;

    throw new RuntimeException('boom');
};

try {
    $read($sharedState['mysqli_unwind_doomed']);
    echo 'mysqli-unwind-not-thrown';
} catch (\Throwable $e) {
    echo 'mysqli-unwind-caught:' . $e->getMessage();
}
