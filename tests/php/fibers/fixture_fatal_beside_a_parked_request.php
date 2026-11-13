<?php

declare(strict_types=1);

// Inner self-request for fibers/test_fatal_keeps_a_parked_requests_destructors
// and fibers/test_fatal_report_runs_no_other_request. Holds an object of its own,
// and one that refers to itself, and then fails while the request that sent it
// is parked: with ?oom=1 by running past memory_limit, with ?timeout=1 by
// running out of time, otherwise with a fatal the engine bails out of.
// ?handler=1 displays that fatal through an output handler that sleeps the
// first time it runs, and marks when it starts and when it returns.

require_once __DIR__ . '/destruct_probe.php';

$own = new OxphpDestructProbe('failed');
$cycle = new OxphpDestructProbe('failed_cycle');
$cycle->cycle = $cycle;

if (isset($_GET['handler'])) {
    ini_set('display_errors', '1');
    ob_start(static function (string $buffer): string {
        static $slept = false;
        if (!$slept) {
            $slept = true;
            OxphpDestructProbe::mark('report_started');
            usleep(300_000);
            OxphpDestructProbe::mark('report_done');
        }

        return $buffer;
    }, 1);
}

if (isset($_GET['oom'])) {
    ini_set('display_errors', '0');
    ini_set('memory_limit', '64M');
    // About 72 MiB in all: past the limit, and short of the hard limit at twice
    // that, which would end the server rather than the request. From PHP 8.4.27
    // / 8.5.12 the engine reports it at the next call, not at the allocation.
    $s = str_repeat('x', 24 << 20);
    $doubled = $s . $s;
    md5('x');
} elseif (isset($_GET['timeout'])) {
    // Not displayed: the request has been answered by then, and displaying the
    // message would be a write to it, which ends the report before the engine
    // flags anything.
    ini_set('display_errors', '0');
    set_time_limit(1);
    $spin = 0;
    while (true) {
        $spin++;
    }
} else {
    oxphp_destruct_probe_fatal();
}

echo 'not reached';
