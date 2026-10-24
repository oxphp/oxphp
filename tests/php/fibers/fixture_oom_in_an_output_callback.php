<?php

declare(strict_types=1);

// Inner self-request for fibers/test_oom_in_an_output_callback_keeps_the_worker.
//
// ?step=oom runs out of memory in an output buffer's callback, on the flush that
// ends the request — after the handler has returned and the shutdown functions
// have run. Neither displayed nor logged: the callback is what would display it.
//
// ?step=probe is the request after it: it says it ran, and so does its shutdown
// function.

$step = $_GET['step'] ?? 'oom';

if ($step === 'probe') {
    register_shutdown_function(static function (): void {
        echo "SHUTDOWN-RAN\n";
    });
    echo "PROBE-RAN\n";
    echo 'worker started: ', sprintf('%.6f', OxPHP\Server\Worker::current()->startTime()), "\n";
    return;
}

ini_set('display_errors', '0');
ini_set('log_errors', '0');

ob_start(static function (string $buffer, int $phase): string {
    if (($phase & PHP_OUTPUT_HANDLER_FINAL) === 0) {
        return $buffer;
    }
    $s = 'x';
    for (;;) {
        $s .= $s;
    }
});

echo "BUFFERED\n";
