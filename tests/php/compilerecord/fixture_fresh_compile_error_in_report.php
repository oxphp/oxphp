<?php

declare(strict_types=1);

// A compile error raised from the error handler that the report of another
// compile runs: that compile, of a file read through a stream filter, records
// its deprecation and reports it once it is over, and the handler includes a
// file written after the worker booted, which OPcache compiles without caching
// and which fails to compile.
// See test_uncached_compile_error_keeps_later_warnings_reported.php.

$path = sys_get_temp_dir() . '/oxphp_compilerecord_error.php';
file_put_contents($path, "<?php\nabstract class OxphpFreshCompileError\n{\n    abstract public function f()\n    {\n    }\n}\n");

set_error_handler(static function () use ($path): bool {
    include $path;
    return true;
}, E_DEPRECATED);

include 'php://filter/read=string.rot13|string.rot13/resource=' . __DIR__ . '/../fibers/fixture_compile_deprecation.inc';

echo "NOT-REACHED\n";
