<?php

declare(strict_types=1);

// A compile error in a file written after the worker booted, which OPcache
// compiles without caching. Each include fails the same way: the class is never
// bound. See test_uncached_compile_error_keeps_later_warnings_reported.php.

$path = sys_get_temp_dir() . '/oxphp_compilerecord_error.php';
file_put_contents($path, "<?php\nabstract class OxphpFreshCompileError\n{\n    abstract public function f()\n    {\n    }\n}\n");

include $path;

echo "NOT-REACHED\n";
