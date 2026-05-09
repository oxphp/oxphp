<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$marker = '/tmp/oxphp-timeout-shutdown-handlers.marker';

$action = $_GET['action'] ?? 'trigger';

if ($action === 'trigger') {
    // Belt-and-suspenders: remove any stale marker from a previous run.
    @unlink($marker);

    register_shutdown_function(function () use ($marker) {
        @file_put_contents($marker, 'timeout_shutdown_ran');
    });

    set_time_limit(1);

    // Busy spin until SIGALRM fires. The interrupt handler runs at opcode
    // boundaries, so calls to microtime() are fine — they just give the VM
    // a chance to check EG(vm_interrupt) between opcodes.
    $end = microtime(true) + 5;
    while (microtime(true) < $end) {
        // intentionally empty - busy spin
    }

    echo 'should never reach here';
    exit;
}

// action=check
// Trigger ran sequentially before this and already completed its bailout,
// so the marker is on disk. Brief sleep to absorb any filesystem-flush
// jitter while staying well under the 2s server wrapper.
usleep(200_000);

$test = new TestCase('timeout_runs_shutdown_handlers', 'timeout');
$exists = is_file($marker);
$content = $exists ? (file_get_contents($marker) ?: '') : '';
if ($exists) {
    unlink($marker);
}

$test->assertTrue('marker file exists after timeout bailout', $exists);
$test->assertSame('marker contents', $content, 'timeout_shutdown_ran');
$test->done();
