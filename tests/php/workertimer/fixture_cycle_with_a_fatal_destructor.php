<?php

declare(strict_types=1);

// Leaves behind a pair of objects that refer to each other and nothing else, so
// that only the cycle collector frees them. A worker runs the collector every
// hundredth request, between requests rather than inside one, so their
// destructor runs where no request is left to end it, and the fatal error it
// ends in ends the worker's serve loop instead.
//
// Before that the destructor registers a shutdown function, as code that defers
// its work to the end of the script does. Registered where no request is
// running, it is left on the thread when the fatal error ends the loop, and the
// worker script's own shutdown runs it. It runs PHP code for longer than any
// limit the check uses, and says in the log when it started, when it had run for
// a second, and whether it reached its end.
//
// ?action=raise_limit leaves no cycle and raises the request's own limit, so that
// the last request before the collector runs is one whose limit differs from the
// one every request starts with.
//
// Driven by tests/scripts/verify_worker_shutdown_is_timed_after_a_bailout.sh, not
// by a suite. Included inside the worker's handler, where $requestCount is in
// scope.

header('Content-Type: application/json');

if (($_GET['action'] ?? '') === 'raise_limit') {
    set_time_limit(7);
    echo json_encode(['raised' => true, 'request_count' => $requestCount]);
    return;
}

if (!class_exists('OxphpWorkerTimerFatalCycle', false)) {
    final class OxphpWorkerTimerFatalCycle
    {
        public ?self $other = null;

        public function __destruct()
        {
            register_shutdown_function(static function (): void {
                $start = microtime(true);
                error_log('oxphp-worker-shutdown-probe: started');
                $saidSecond = false;
                while (($ran = microtime(true) - $start) < 8.0) {
                    if (!$saidSecond && $ran >= 1.0) {
                        error_log('oxphp-worker-shutdown-probe: still running after 1s');
                        $saidSecond = true;
                    }
                }
                error_log(sprintf('oxphp-worker-shutdown-probe: finished after %.1fs', $ran));
            });
            trigger_error('oxphp-worker-timer: a destructor the collector ran ended in a fatal error', E_USER_ERROR);
        }
    }
}

$first = new OxphpWorkerTimerFatalCycle();
$second = new OxphpWorkerTimerFatalCycle();
$first->other = $second;
$second->other = $first;
unset($first, $second);

echo json_encode(['cycle_left' => true, 'request_count' => $requestCount]);
