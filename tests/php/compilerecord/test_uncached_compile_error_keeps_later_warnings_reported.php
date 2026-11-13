<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// From PHP 8.5 OPcache records the errors raised while it compiles a file, and
// where it compiles one without caching it, it ends that recording only when the
// compile returns. A file changed less than file_update_protection seconds
// before the request time is compiled that way, and a worker's request time is
// its boot, so every file written after the boot is. A compile error in one
// leaves the thread recording every later error instead of reporting it — no
// display, no log, no error handler — and a worker runs every request it serves
// on one thread. The first inner request fatals that way; this request, parked
// on the same thread while it ran, must still have its warnings reported after.
//
// The second inner request fatals the same way in the error handler that the
// report of another compile runs. That compile has given back its arena by
// then, and the bailout must leave the arena alone: given back a second time, it
// aborts the server under the system allocator this profile runs with.
//
// Before 8.5 OPcache does not record around these compiles and the handler runs
// inside the other compile: both pass for nothing there.

// Cleared so the warnings below reach the handlers installed for them rather
// than one the last request on this worker left standing.
set_error_handler(null);
set_exception_handler(null);

// Null when the inner request could not be sent.
$inner = static function (string $path): ?string {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return null;
    }
    stream_set_timeout($sock, 10);
    fwrite($sock, "GET /tests/compilerecord/{$path} HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
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

// The premise: a file written now is compiled without caching, one the worker
// found on disk at boot is cached.
$fresh = sys_get_temp_dir() . '/oxphp_compilerecord_fresh.php';
file_put_contents($fresh, "<?php\nreturn 1;\n");
include $fresh;
$freshCached = opcache_is_script_cached($fresh);
$thisCached = opcache_is_script_cached(__FILE__);

$compileError = $inner('fixture_fresh_compile_error.php');
$afterCompileError = $reportedAfter('after the compile error');
$reportFatal = $inner('fixture_fresh_compile_error_in_report.php');
$afterReportFatal = $reportedAfter('after the compile error in the report');

$t = new TestCase('uncached_compile_error_keeps_later_warnings_reported', 'compilerecord');

$t->assertTrue('a file found at boot is cached', $thisCached);
$t->assertTrue('a file written after the boot is compiled without caching', !$freshCached);

$t->assertTrue('the inner request connected', $compileError !== null);
$t->assertContains('the inner request fataled in the compile', (string) $compileError, 'cannot contain body');
$t->assertNotContains('and nothing past the include ran', (string) $compileError, 'NOT-REACHED');
$t->assertTrue('a warning raised after it on the same thread reaches its error handler', $afterCompileError);

$t->assertTrue('the second inner request connected', $reportFatal !== null);
$t->assertContains(
    'it fataled in the compile its error handler started',
    (string) $reportFatal,
    'cannot contain body'
);
$t->assertNotContains('and nothing past the include ran', (string) $reportFatal, 'NOT-REACHED');
$t->assertTrue('the worker survived it, and a later warning reaches its error handler', $afterReportFatal);

$t->done();
