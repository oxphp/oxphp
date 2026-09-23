<?php

declare(strict_types=1);

// A session is written even when the request's final flush throws.
//
// An output callback left open is ended by the flush that closes the request,
// and one that throws there leaves an exception that nothing under the worker's
// loop reports or rethrows. Left pending, it turned every call into userland
// that came after it into one that returns at once without running — the save
// handler's write() and close() among them, so the session's data was lost and
// a handler that locks its store held the lock until it expired. The engine
// reports such an exception at that step of its own request shutdown, before
// the session is written; the probe on the next line reads that the handler was
// written and closed.

require_once __DIR__ . '/session_flush_throw_probe.php';

set_error_handler(null);
set_exception_handler(null);

OxphpSessionFlushThrowProbe::$written = null;
OxphpSessionFlushThrowProbe::$closed = false;
OxphpSessionFlushThrowProbe::$callbackRan = false;
OxphpSessionFlushThrowProbe::$callbackRanBeforeWrite = null;

// Not registered for shutdown: that registers a shutdown function which writes
// the session before the final flush, and the write under test is the one the
// request's own end makes after it.
session_set_save_handler(new OxphpSessionFlushThrowHandler(), false);
session_id('oxphpsessionflushthrow');
session_start();
$_SESSION['marker'] = 'kept';

ob_start(static function (string $buffer): string {
    OxphpSessionFlushThrowProbe::$callbackRan = true;
    throw new RuntimeException('thrown by an output callback in the final flush');
});

echo 'set';
