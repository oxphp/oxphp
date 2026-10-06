<?php

declare(strict_types=1);

// Inner self-request and runner line for the tests of memory_limit after a fatal
// raised deep in a call stack: fibers/test_memory_limit_holds_after_a_deep_fatal,
// fibers/test_memory_limit_holds_after_a_deep_fatal_on_its_own and
// fibers/test_memory_limit_retires_a_worker_it_cannot_return_to.
//
// ?step=fatal runs out of memory 400,000 calls down. From PHP 8.5 the engine
// takes a backtrace of a fatal, every frame of it, unless fatal_error_backtraces
// is off, and it takes it while PHP's allocator has set memory_limit aside to
// report the error: that backtrace is allocated with no limit, a couple of
// hundred megabytes of it at this depth.
// Freeing it at the end of the request does not hand that memory back to the
// allocator's accounting by itself, and while the allocator counts more than
// the limit as in use, it stops enforcing the limit at all.
//
// ?step=stash does the same and, from a shutdown function, which runs while the
// backtrace is still held and the allocator therefore enforces no limit, puts
// 200 MiB into the worker's shared state, where it outlives the request, and
// records what it did in a file for the line that follows it. Only on PHP 8.5:
// before it there is no backtrace to hold the limit off.
//
// ?step=probe reports how much the allocator counts as in use, then asks for
// 200 MiB more than a 128M memory_limit could ever grant. Where the limit is
// enforced that ends the request in a fatal error; where it is not, the request
// gets the memory and says so.
//
// The fatal is raised by a string concatenation in the request's own code, not
// inside an internal function, so the worker's handling of a request ended in
// the middle of an internal function's call stays out of it. Neither displayed
// nor logged: at this depth the backtrace runs to tens of megabytes of text,
// which would go into the response or the log. The engine renders that text
// either way and frees it at once; it is the backtrace, not its text, that
// outlives the request.
//
// Declared conditionally for the reason fixture_closure_fatal.php gives.

if (!function_exists('oxphp_deep_backtrace_fatal_descend')) {
    function oxphp_deep_backtrace_fatal_descend(int $depth): void
    {
        if ($depth > 0) {
            oxphp_deep_backtrace_fatal_descend($depth - 1);
            return;
        }
        $s = 'x';
        for (;;) {
            $s .= $s;
        }
    }
}

set_error_handler(null);
error_clear_last();

$step = $_GET['step'] ?? 'fatal';

if ($step === 'probe') {
    register_shutdown_function(static function (): void {
        echo 'last error: ', error_get_last()['message'] ?? 'none', "\n";
    });
    echo 'real: ', memory_get_usage(true), "\n";
    echo 'limit: ', ini_parse_quantity((string) ini_get('memory_limit')), "\n";
    echo 'worker started: ', sprintf('%.6f', OxPHP\Server\Worker::current()->startTime()), "\n";
    echo 'stash: ', isset($sharedState['deep_backtrace_stash']) ? 'present' : 'absent', "\n";
    $big = str_repeat('x', 200 << 20);
    echo 'NOT-ENFORCED: ', strlen($big), "\n";
    return;
}

ini_set('display_errors', '0');
ini_set('log_errors', '0');

$stash = $step === 'stash';
register_shutdown_function(static function () use ($stash, &$sharedState): void {
    $lastError = error_get_last()['message'] ?? 'none';
    $workerStarted = OxPHP\Server\Worker::current()->startTime();
    echo 'last error: ', $lastError, "\n";
    echo 'worker started: ', sprintf('%.6f', $workerStarted), "\n";
    if (!$stash) {
        return;
    }
    if (PHP_VERSION_ID >= 80500) {
        $sharedState['deep_backtrace_stash'] = str_repeat('x', 200 << 20);
    }
    file_put_contents('/tmp/oxphp-deep-backtrace-stash', json_encode([
        'last_error'     => $lastError,
        'worker_started' => $workerStarted,
        'stashed'        => strlen($sharedState['deep_backtrace_stash'] ?? ''),
    ]));
});

oxphp_deep_backtrace_fatal_descend(400_000);

echo "NOT-REACHED\n";
