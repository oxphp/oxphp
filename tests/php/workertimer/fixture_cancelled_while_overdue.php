<?php

declare(strict_types=1);

// Inner request for
// workertimer/test_overdue_request_cancelled_by_a_recycle_runs_its_shutdown_functions:
// sets one second and parks for five, so its limit is over while it waits, and
// the worker is told to exit before the sleep ends, which cancels it where it
// waits. Its shutdown functions report to a file, because the worker's memory
// goes with the worker.

require_once __DIR__ . '/timer_probe.php';

OxphpWorkerTimerProbe::start('overdue');

register_shutdown_function(static function (): void {
    // Registered after the probe's own. A timeout still pending from the resume
    // would end the first of them to make an internal call — the probe's — and
    // none from there on would run, this one included.
    file_put_contents('/tmp/oxphp-workertimer-cancelled.txt', "first\n", FILE_APPEND);
});
register_shutdown_function(static function (): void {
    file_put_contents('/tmp/oxphp-workertimer-cancelled.txt', json_encode([
        'status' => connection_status(),
        'message' => error_get_last()['message'] ?? '',
    ]) . "\n", FILE_APPEND);
});

set_time_limit(1);

// Hooked in this profile: parks this request for the whole five seconds.
usleep(5_000_000);

OxphpWorkerTimerProbe::$runs['overdue']['woke'] = true;
echo 'woke';
