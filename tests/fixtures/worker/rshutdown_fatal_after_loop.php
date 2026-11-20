<?php

// Run by worker_entry.php after the loop, once recycle/test_rshutdown_fatal_arm
// has armed it: a fatal error raised inside the session extension's request
// shutdown, which a worker's thread reaches only as it exits. The trap is the
// one bailout/fixture_rshutdown_fatal sets, adapted from PHP's own test for
// GHSA-g3vh-838c-65v7: once $_SESSION is unset, the session's own reference to
// its data is the last thing holding the stream, so the stream is closed when
// the session's RSHUTDOWN drops that reference, and stream_close() raises the
// fatal there.
//
// A session that failed to start would leave $_SESSION an ordinary array, and
// the unset() would close the stream right here instead; that is written down
// for the test to tell apart.

final class OxphpRshutdownFatalStream
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
        file_put_contents(sys_get_temp_dir() . '/oxphp-rshutdown-fatal-worker.closed', 'closed');
        trigger_error('worker session shutdown bailout', E_USER_ERROR);
    }
}

// The handlers the last request's test set are still installed: they outlive
// their request in a worker. One would turn the fatal into an exception of its
// own, or a warning into one.
set_error_handler(null);
set_exception_handler(null);

stream_wrapper_register('shutdown-fatal', OxphpRshutdownFatalStream::class);
if (!session_start() || session_status() !== PHP_SESSION_ACTIVE) {
    file_put_contents(sys_get_temp_dir() . '/oxphp-rshutdown-fatal-worker.nosession', 'no session');
    return;
}
$_SESSION['stream'] = fopen('shutdown-fatal://session', 'w');
unset($_SESSION);
