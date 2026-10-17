<?php

declare(strict_types=1);

/**
 * Include a generated file of $count functions and return its path.
 *
 * OPcache gives each function of a file it caches its own slots in every PHP
 * thread's table of per-thread pointers (the runtime cache, the static
 * variables), numbered in the order it caches them. A file this thread
 * includes next gets slots this many numbers further on, well past the end of
 * the table a task's thread was started with.
 */
function asyncdecl_bulk(string $tag, int $count = 10000): string
{
    $path = sys_get_temp_dir() . "/asyncdecl_bulk_$tag.php";
    if (!is_file($path)) {
        $code = "<?php\n\ndeclare(strict_types=1);\n\n";
        for ($i = 0; $i < $count; $i++) {
            $code .= "function asyncdecl_bulk_{$tag}_$i(): ?int { static \$s = null; return \$s; }\n";
        }
        $tmp = "$path." . getmypid();
        file_put_contents($tmp, $code);
        rename($tmp, $path);
    }
    require_once $path;
    return $path;
}
