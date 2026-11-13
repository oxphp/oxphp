<?php

declare(strict_types=1);

// A compile that succeeds and fatals afterwards, in the report of a deprecation
// it raised: the error handler that report runs fatals. From PHP 8.5 the engine
// reports the errors of a compile once the compile is over, by which time it has
// given back the arena it compiled in. Read through a stream filter, so that
// OPcache hands the file to the engine to compile.
// See test_compile_error_keeps_later_warnings_reported.php.

// Declared conditionally for the reason fixture_fatal.php gives, and called
// twice for the same reason: a class declared at run time fatals with
// E_COMPILE_ERROR once its name is taken.
if (!function_exists('oxphp_report_fatal_declare')) {
    function oxphp_report_fatal_declare(): void
    {
        class OxphpReportFatal
        {
        }
    }
}

set_error_handler(static function (): bool {
    oxphp_report_fatal_declare();
    oxphp_report_fatal_declare();
    return true;
}, E_DEPRECATED);

include 'php://filter/read=string.rot13|string.rot13/resource=' . __DIR__ . '/fixture_compile_deprecation.inc';

echo "NOT-REACHED\n";
