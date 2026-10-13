<?php

declare(strict_types=1);

// Parks before any session is open on the worker, so it is already on the
// worker when another client's session is opened there; then, woken inside that
// hold, gives itself a one-second limit and calls session_start(), which has to
// wait for the other session to close.
//
// The limit is meant to end it inside that wait, so it reports from a shutdown
// function, into the worker's shared state: when it ended, whether the limit is
// what ended it, and whether it ever got a session.

set_error_handler(null);
set_exception_handler(null);

register_shutdown_function(static function () use (&$sharedState): void {
    $sharedState['session_wait_time_limit']['ended_at'] = microtime(true);
    $sharedState['session_wait_time_limit']['timed_out'] = (connection_status() & CONNECTION_TIMEOUT) !== 0;
    $sharedState['session_wait_time_limit']['session_status'] = session_status();
});

// A request the worker has not taken yet would wait in the queue while the
// session is open and never reach session_start() inside the hold, so this one
// is taken first and parks.
$sharedState['session_wait_time_limit']['parked_at'] = microtime(true);
oxphp_sleep(0.4);
$sharedState['session_wait_time_limit']['resumed_at'] = microtime(true);

set_time_limit(1);
$sharedState['session_wait_time_limit']['limit_set_at'] = microtime(true);
session_start();

// Reached only if the limit did not end the request at the call above.
$sharedState['session_wait_time_limit']['returned_at'] = microtime(true);
session_write_close();
echo 'returned';
