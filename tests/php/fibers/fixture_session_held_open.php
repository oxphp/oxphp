<?php

declare(strict_types=1);

// Opens a session under the client's cookie and keeps it open across a park of
// a second, or of ?hold= seconds, then reports when it opened it, when it was
// done with it, when it had closed it, and how much CPU time the worker thread
// spent while it was open.
//
// The times are what the tests that put another request into this window
// compare that request against. The CPU time is for the one that leaves
// requests waiting for the session through the window: a wait that kept the
// worker busy instead of parking would show here.

set_error_handler(null);
set_exception_handler(null);

// PHP 8.6 turns session.use_strict_mode on by default, and strict mode swaps an
// id the store has never seen for a fresh one. The id here is the client's
// cookie, and the test reads it back, so it has to be kept.
ini_set('session.use_strict_mode', '0');

// The CPU time of the thread this runs on, in seconds, from its own stat line.
// The command name in it is parenthesised and may hold spaces, so the fields
// are counted from after the closing parenthesis: utime and stime are the 14th
// and 15th fields of the line, which makes them the 12th and 13th after it.
$threadCpu = static function (): float {
    $stat = (string) file_get_contents('/proc/thread-self/stat');
    $fields = explode(' ', substr($stat, strrpos($stat, ')') + 2));

    return ((int) ($fields[11] ?? 0) + (int) ($fields[12] ?? 0)) / 100;
};

session_start();
$_SESSION['who'] = 'the request that holds it';
$openedAt = microtime(true);
$cpuBefore = $threadCpu();

oxphp_sleep((float) ($_GET['hold'] ?? 1.0));

$cpuAfter = $threadCpu();
$doneAt = microtime(true);
$id = session_id();
session_write_close();
$closedAt = microtime(true);

header('Content-Type: application/json');
echo json_encode([
    'id' => $id,
    'opened_at' => $openedAt,
    'done_at' => $doneAt,
    'closed_at' => $closedAt,
    'cpu_during_hold' => $cpuAfter - $cpuBefore,
]);
