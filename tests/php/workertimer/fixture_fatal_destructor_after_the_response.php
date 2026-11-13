<?php

declare(strict_types=1);

// Puts an object into $_GET whose destructor puts another one back. The first is
// let go of as the request ends, inside its fiber; the second only by the
// worker's own end of the request, after the response has gone out and outside
// any fiber, where no request is left to end. The fatal error its destructor
// ends in leaves the worker's serve loop by a long jump. A fatal in the cycle
// collection the worker runs every hundredth request would not: that one retires
// the worker through the end of the loop.
//
// Before that the destructor registers a shutdown function, as code that defers
// its work to the end of the script does. Registered where no request is
// running, it is left on the thread when the fatal error ends the loop, and the
// worker script's own shutdown runs it. It runs PHP code for longer than any
// limit the check uses, and says in the log when it started, when it had run for
// a second, and whether it reached its end.
//
// The request raises its own limit first, so that a fatal that names it rather
// than the profile's two seconds would say the worker script was left on that
// request's limit. And it arms the code after the oxphp_worker() call in the
// worker entry script, which the long jump skips: ?action=after_loop, served by
// the worker that replaces this one, says whether that code ran, and whether the
// mark it takes down on its way is still there.
//
// Driven by tests/scripts/verify_worker_shutdown_is_timed_after_a_bailout.sh, not
// by a suite. Included inside the worker's handler, where $requestCount is in
// scope.

require_once __DIR__ . '/../engine_fatal.php';
require_once __DIR__ . '/../fibers/after_loop_probe.php';

header('Content-Type: application/json');

if (($_GET['action'] ?? '') === 'after_loop') {
    clearstatcache();
    echo json_encode([
        'request_count' => $requestCount,
        'armed' => is_file(OxphpAfterLoop::armed()),
        'after_loop' => is_file(OxphpAfterLoop::ran()) ? file_get_contents(OxphpAfterLoop::ran()) : null,
    ]);
    return;
}

if (!class_exists('OxphpWorkerTimerFatalAfterTheResponse', false)) {
    final class OxphpWorkerTimerFatalAfterTheResponse
    {
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
            engine_fatal('OxphpWorkerTimerDestructorFatal');
        }
    }

    final class OxphpWorkerTimerPutsBack
    {
        public function __destruct()
        {
            $_GET['put_back'] = new OxphpWorkerTimerFatalAfterTheResponse();
        }
    }
}

set_time_limit(7);
OxphpAfterLoop::arm();
$_GET['puts_back'] = new OxphpWorkerTimerPutsBack();

echo json_encode(['put_back' => true, 'request_count' => $requestCount]);
