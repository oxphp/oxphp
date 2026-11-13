<?php

declare(strict_types=1);

// Inner self-request for
// fibers/test_oom_before_the_superglobals_are_released_spares_their_callbacks.
//
// Each step leaves a stream in $_GET with a user filter on it, whose onClose()
// leaves a mark as the stream is closed — which happens as the worker drops the
// request's superglobals, after a step of the request's end before that has run
// out of memory.
//
// Each also dispatches a task that busies the profile's one async thread and,
// once it is running, a second one that holds something by reference. The
// second has not started when the request ends, so the request's end releases
// its closure, and what it holds with it, before the superglobals.
//
// ?step=task has the second task hold an object whose destructor runs out of
// memory. It leaves a mark first, so the test can tell a destructor that ran
// from one that never did.
//
// ?step=session has it hold another filtered stream, and leaves about 36 MiB in
// its session under a 64 MiB memory_limit: serialising it for the files handler
// as the request ends runs out of memory, in C code, before the task's closure
// is released.
//
// ?step=probe is a request after it: it says it ran, that a callback still runs,
// and which worker served it, and removes the session file the session step
// wrote.

$step = (string) ($_GET['step'] ?? '');
$run = preg_replace('/[^0-9a-f]/', '', (string) ($_GET['run'] ?? ''));
$base = sys_get_temp_dir() . '/oxphp-oom-release-' . $run;
$sessionId = 'oxphpoomrelease' . $run;

if ($step === 'probe') {
    echo "PROBE-RAN\n";
    echo 'callback: ', json_encode(array_map(static fn (int $n): int => $n * 2, [1, 2])), "\n";
    echo 'worker started: ', sprintf('%.6f', OxPHP\Server\Worker::current()->startTime()), "\n";
    $parts = explode(';', (string) session_save_path());
    $dir = end($parts) ?: sys_get_temp_dir();
    @unlink($dir . '/sess_' . $sessionId);
    return;
}

// Cleared and set here rather than trusted: handlers live for the life of the
// worker in worker mode.
set_error_handler(null);
set_exception_handler(null);
ini_set('display_errors', '0');
ini_set('log_errors', '0');

if (!class_exists('OxphpOomReleaseRecordsClose', false)) {
    class OxphpOomReleaseRecordsClose extends php_user_filter
    {
        public function onClose(): void
        {
            file_put_contents((string) $this->params, 'closed');
        }

        /** @param resource $in @param resource $out */
        public function filter($in, $out, &$consumed, bool $closing): int
        {
            return PSFS_PASS_ON;
        }
    }
}

// The filter map is per thread and this worker keeps it for its whole life, so
// once a request on this worker has registered the name it stays registered.
if (!in_array('oxphp-oom-release-records-close', stream_get_filters(), true)) {
    stream_filter_register('oxphp-oom-release-records-close', OxphpOomReleaseRecordsClose::class);
}

$filtered = static function (string $marker) {
    $stream = fopen('php://memory', 'r+');
    stream_filter_append($stream, 'oxphp-oom-release-records-close', STREAM_FILTER_ALL, $marker);

    return $stream;
};

$_GET['stream'] = $filtered("$base.closed");

/** Leaves $held to the closure of a task that has not started. */
$leaveToAnUnstartedTask = static function (mixed $held) use ($base): bool {
    oxphp_async(static function () use ($base): int {
        file_put_contents("$base.blocker", 'running');
        // On the CPU, not parked: a task that parks hands the thread to the
        // next task, which would then start.
        $end = microtime(true) + 3.0;
        while (microtime(true) < $end) {
        }

        return 1;
    });
    // usleep() parks this request meanwhile.
    for ($i = 0; $i < 100 && !is_file("$base.blocker"); $i++) {
        usleep(20_000);
    }
    if (!is_file("$base.blocker")) {
        return false;
    }
    $box = null;
    oxphp_async(static function () use (&$box): int {
        return 2;
    });
    // Assigned after the dispatch, which checks only what the closure captured
    // then; once this returns, the closure is the only holder.
    $box = $held;

    return true;
};

switch ($step) {
    case 'task':
        // Anonymous: a named class declared here would be declared again by the
        // next request on this worker to include this file.
        $left = $leaveToAnUnstartedTask(new class ("$base.ran") {
            public function __construct(private string $marker)
            {
            }

            public function __destruct()
            {
                file_put_contents($this->marker, 'ran');
                $s = 'x';
                for (;;) {
                    $s .= $s;
                }
            }
        });
        echo $left ? "ARMED\n" : "BLOCKER-NEVER-RAN\n";
        return;

    case 'session':
        ini_set('session.save_handler', 'files');
        // PHP 8.6 turns strict mode on by default, which would swap this id for
        // a fresh one, and the probe removes the file by it.
        ini_set('session.use_strict_mode', '0');
        ini_set('memory_limit', '64M');
        session_id($sessionId);
        session_start();
        $_SESSION['big'] = str_repeat('x', 36 << 20);
        $left = $leaveToAnUnstartedTask($filtered("$base.task-closed"));
        echo $left ? "ARMED\n" : "BLOCKER-NEVER-RAN\n";
        return;

    default:
        echo "FAIL: unknown step\n";
        return;
}
