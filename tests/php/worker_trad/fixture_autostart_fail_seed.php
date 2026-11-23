<?php

declare(strict_types=1);

// Prepares a session whose decoding fails, for
// test_thread_retires_after_a_failed_request_startup in the tradautostart
// profile: the next request that sends its cookie bails out of its startup.
$_SESSION['thrower'] = new WorkerTradThrowOnWake();

header('Content-Type: application/json');
echo json_encode([
    'worker' => oxphp_worker_id(),
    'sid' => session_id(),
    'thread' => readlink('/proc/thread-self'),
]);
