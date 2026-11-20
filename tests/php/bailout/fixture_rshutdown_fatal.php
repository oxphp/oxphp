<?php

// A fatal error raised inside the session extension's request shutdown,
// adapted from PHP's own test for GHSA-g3vh-838c-65v7
// (sapi/fpm/tests/ghsa-g3vh-838c-65v7.phpt).
//
// Once $_SESSION is unset, the session's own copy of its data is the last
// thing holding the stream, so the stream is closed when the session's
// RSHUTDOWN releases that copy, outside the guard the session puts around its
// write, and the wrapper's stream_close() raises the fatal there. From PHP
// 8.4.28 / 8.5.13 the engine ends the process at that point unless the server
// catches it.
//
// stream_close() leaves a marker first, so the test that runs next can tell
// the trap fired.

class ShutdownFatalStream
{
    public $context;

    public function stream_open($path, $mode, $options, &$openedPath)
    {
        return true;
    }

    public function stream_write($data)
    {
        return strlen($data);
    }

    public function stream_close()
    {
        file_put_contents('/tmp/oxphp-rshutdown-fatal-marker', 'closed');
        trigger_error('session shutdown bailout', E_USER_ERROR);
    }
}

if (is_file('/tmp/oxphp-rshutdown-fatal-marker')) {
    unlink('/tmp/oxphp-rshutdown-fatal-marker');
}

stream_wrapper_register('shutdown-fatal', ShutdownFatalStream::class);
session_start();
$_SESSION['stream'] = fopen('shutdown-fatal://session', 'w');
unset($_SESSION);

echo "fixture done\n";
