<?php

declare(strict_types=1);

// A compile error in a file OPcache hands to the engine to compile: it caches
// files on disk and in phar archives only, and this one is read through a
// stream filter that gives it back unchanged. From PHP 8.5 the engine records the errors of such a compile
// to report them once it is over, and a compile error ends it before that.
// See test_compile_error_keeps_later_warnings_reported.php.

include 'php://filter/read=string.rot13|string.rot13/resource=' . __DIR__ . '/fixture_compile_error.inc';

echo "NOT-REACHED\n";
