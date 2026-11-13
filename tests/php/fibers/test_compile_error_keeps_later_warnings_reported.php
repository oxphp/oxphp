<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// From PHP 8.5 the engine records the errors raised while it compiles a file
// and reports them once the compile is over, and a compile error ends the
// compile before that. Left recording, the thread records every error after it
// instead of reporting it — no display, no log, no error handler — and a worker
// runs every request it serves on one thread. The inner request below fatals in
// such a compile on this worker, so this request, parked on the same thread
// while it ran, must still have its warnings reported afterwards.
//
// The file is read through a stream filter, and OPcache hands such a file to the
// engine to compile without recording anything itself, which leaves the
// recording to the engine. A file OPcache compiles without caching while it
// records is the compilerecord profile's. Before 8.5 the engine does not record,
// and this passes for nothing.
//
// The report of a compile that succeeds runs the user's error handler, after the
// compile has given its arena back, and a fatal raised there must leave that
// arena alone. The second inner request fatals that way. Giving the arena back a
// second time goes unnoticed by PHP's own allocator and crashes the server with
// USE_ZEND_ALLOC=0. Before 8.5 the handler runs inside the compile, and that
// request fatals in it instead.
//
// A compile of such a file that succeeds has to report what it recorded: that
// half is read from this request.

// Cleared so the warning below reaches the handler installed for it rather than
// one the last request on this worker left standing.
set_error_handler(null);
set_exception_handler(null);

// Null when the inner request could not be sent.
$inner = static function (string $path): ?string {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return null;
    }
    stream_set_timeout($sock, 10);
    fwrite($sock, "GET /tests/fibers/{$path} HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    // Hooked: parks this request while the worker serves the inner one.
    $resp = (string) stream_get_contents($sock);
    fclose($sock);
    return $resp;
};
$reportedAfter = static function (string $message): bool {
    $reported = false;
    set_error_handler(static function () use (&$reported): bool {
        $reported = true;
        return true;
    });
    trigger_error($message, E_USER_WARNING);
    restore_error_handler();
    return $reported;
};

$compileError = $inner('fixture_compile_error_outside_opcache.php');
$afterCompileError = $reportedAfter('after the compile error');
$reportFatal = $inner('fixture_compile_report_fatal.php');
$afterReportFatal = $reportedAfter('after the fatal in the report');

$t = new TestCase('compile_error_keeps_later_warnings_reported', 'fibers');

$t->assertTrue('the inner request connected', $compileError !== null);
// The fatal is the compile error, raised by the compile, not by anything the
// request ran afterwards.
$t->assertContains('the inner request fataled in the compile', (string) $compileError, 'cannot contain body');
$t->assertNotContains('and nothing past the include ran', (string) $compileError, 'NOT-REACHED');
$t->assertTrue('a warning raised after it on the same thread reaches its error handler', $afterCompileError);

$t->assertTrue('the second inner request connected', $reportFatal !== null);
$t->assertContains(
    'it fataled in the error handler the deprecation ran',
    (string) $reportFatal,
    'OxphpReportFatal'
);
$t->assertNotContains('and nothing past the include ran', (string) $reportFatal, 'NOT-REACHED');
$t->assertTrue('the worker survived it, and a later warning reaches its error handler', $afterReportFatal);

$seen = [];
set_error_handler(static function (int $errno, string $errstr) use (&$seen): bool {
    $seen[] = $errstr;
    return true;
});
include 'php://filter/read=string.rot13|string.rot13/resource=' . __DIR__ . '/fixture_compile_deprecation.inc';
trigger_error('after the compile that succeeded', E_USER_WARNING);
restore_error_handler();

$t->assertTrue(
    'a compile that succeeds reports the deprecation it raised',
    preg_grep('/Implicitly marking parameter \$x as nullable/', $seen) !== []
);
$t->assertTrue(
    'and a warning raised after it reaches its error handler',
    in_array('after the compile that succeeded', $seen, true)
);

$t->done();
