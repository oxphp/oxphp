<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$marker = '/tmp/oxphp-timeout-connection-status.marker';

$action = $_GET['action'] ?? 'trigger';

if ($action === 'trigger') {
    @unlink($marker);

    ignore_user_abort(true);
    register_shutdown_function(function () use ($marker) {
        @file_put_contents($marker, (string) connection_status());
    });

    set_time_limit(1);

    // Busy spin until SIGALRM fires.
    $end = microtime(true) + 5;
    while (microtime(true) < $end) {
        // intentionally empty - busy spin
    }

    echo 'should never reach here';
    exit;
}

// action=check
// Trigger already completed its bailout sequentially. Brief settle.
usleep(200_000);

$test = new TestCase('timeout_sets_connection_status', 'timeout');
$exists = is_file($marker);
$raw = $exists ? (file_get_contents($marker) ?: '') : '';
if ($exists) {
    unlink($marker);
}

$status = (int) $raw;
// PHP_CONNECTION_TIMEOUT == 2 (bit 1). The bailout path OR's it into
// PG(connection_status) before unwinding, so the shutdown function should
// observe the bit set.
$timeoutBitSet = (($status & 2) !== 0);

$test->assertTrue('marker file exists after timeout bailout', $exists);
$test->assertTrue("connection_status has PHP_CONNECTION_TIMEOUT bit set (raw='$raw')", $timeoutBitSet);
$test->done();
