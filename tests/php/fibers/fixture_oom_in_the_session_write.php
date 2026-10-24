<?php

declare(strict_types=1);

// Inner self-request for fibers/test_oom_in_the_session_write_stays_with_its_request.
//
// ?step=oom leaves about 36 MiB in its session for the end of the request to
// write, under a 64 MiB memory_limit: serialising it for the files handler takes
// as much again, past the limit. Neither displayed nor logged.
//
// ?step=probe is the request after it: it says it ran and on which worker, and
// removes the session file the first one wrote.

// Not a constant: in worker mode constants outlive the request, and this file
// runs twice on the same worker.
$sessionId = 'oxphpoomsessionwrite';

$step = $_GET['step'] ?? 'oom';

if ($step === 'probe') {
    echo "PROBE-RAN\n";
    echo 'worker started: ', sprintf('%.6f', OxPHP\Server\Worker::current()->startTime()), "\n";
    $parts = explode(';', (string) session_save_path());
    $dir = end($parts) ?: sys_get_temp_dir();
    @unlink($dir . '/sess_' . $sessionId);
    return;
}

// Cleared and set here rather than trusted: handlers live for the life of the
// worker in worker mode, and the save handler has to be the files one, whose
// write runs no PHP code.
set_error_handler(null);
set_exception_handler(null);
ini_set('display_errors', '0');
ini_set('log_errors', '0');
ini_set('session.save_handler', 'files');
// PHP 8.6 turns strict mode on by default, which would swap this id for a fresh
// one, and the probe removes the file by it.
ini_set('session.use_strict_mode', '0');
ini_set('memory_limit', '64M');

session_id($sessionId);
session_start();
$_SESSION['big'] = str_repeat('x', 36 << 20);

echo "SESSION-SET\n";
